use super::*;
use std::ffi::OsStr;

#[test]
fn config_store_load_refreshes_disk_and_preserves_last_known_good() {
    let temp = tempfile::tempdir().unwrap();
    let store = ExternalCliConfigStore::new(temp.path());
    let original = store.load();

    let mut updated = original.clone();
    updated.default_runner_id = DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string();
    save_config_to_disk(&store.file_path, &updated).unwrap();
    assert_eq!(
        store.load().default_runner_id,
        DEFAULT_CLAUDE_CODE_RUNNER_ID
    );

    std::fs::write(&store.file_path, b"{partial").unwrap();
    assert_eq!(
        store.load().default_runner_id,
        DEFAULT_CLAUDE_CODE_RUNNER_ID,
        "an invalid concurrent write must not discard the worker's last known-good config"
    );
}

fn unused_model_tx() -> tokio::sync::mpsc::Sender<ExternalCliWorkerModelUpdateRequest> {
    tokio::sync::mpsc::channel(1).0
}
struct ExternalCliEnvGuard {
    _env_guard: tokio::sync::MutexGuard<'static, ()>,
    _data_dir_guard: std::sync::MutexGuard<'static, ()>,
}

fn external_cli_env_guard() -> ExternalCliEnvGuard {
    let env_guard = EXTERNAL_CLI_TEST_ENV_LOCK.blocking_lock();
    let data_dir_guard = crate::test_env::bifrost_data_dir_lock();
    ExternalCliEnvGuard {
        _env_guard: env_guard,
        _data_dir_guard: data_dir_guard,
    }
}

async fn external_cli_env_guard_async() -> ExternalCliEnvGuard {
    let env_guard = external_cli_test_env_lock().await;
    let data_dir_guard = crate::test_env::bifrost_data_dir_lock();
    ExternalCliEnvGuard {
        _env_guard: env_guard,
        _data_dir_guard: data_dir_guard,
    }
}

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

/// Overrides both data-dir sources while `external_cli_env_guard_async` owns
/// the shared data-dir lock. This keeps worker-runtime fixtures out of the
/// developer's real Bifrost directory without trying to acquire the lock a
/// second time.
struct ExternalCliDataDirOverride {
    previous_env: Option<String>,
    previous_static: PathBuf,
}

impl ExternalCliDataDirOverride {
    fn set(path: &Path) -> Self {
        let previous_env = std::env::var("BIFROST_DATA_DIR").ok();
        let previous_static = bifrost_storage::data_dir();
        unsafe {
            std::env::set_var("BIFROST_DATA_DIR", path);
        }
        bifrost_storage::set_data_dir(path.to_path_buf());
        Self {
            previous_env,
            previous_static,
        }
    }
}

impl Drop for ExternalCliDataDirOverride {
    fn drop(&mut self) {
        unsafe {
            match self.previous_env.take() {
                Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
                None => std::env::remove_var("BIFROST_DATA_DIR"),
            }
        }
        bifrost_storage::set_data_dir(self.previous_static.clone());
    }
}

impl EnvGuard {
    fn set_str(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }

    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, previous }
    }
}

#[tokio::test]
async fn main_broker_routes_guide_model_update_and_stop_to_the_main_process_registry() {
    let env_guard = crate::worker_runtime::im_broker::broker_test_lock().await;
    let data_dir_guard = crate::test_env::bifrost_data_dir_lock();
    let _registry_guard = ExternalCliEnvGuard {
        _env_guard: env_guard,
        _data_dir_guard: data_dir_guard,
    };
    let _worker = EnvGuard::set_str("BIFROST_IM_GATEWAY_WORKER", "1");
    let endpoint = crate::worker_runtime::im_broker::ensure_main_broker()
        .await
        .expect("start main broker");
    let _addr = EnvGuard::set_str(
        crate::worker_runtime::im_broker::BROKER_ADDR_ENV,
        &endpoint.addr,
    );
    let _token = EnvGuard::set_str(
        crate::worker_runtime::im_broker::BROKER_TOKEN_ENV,
        &endpoint.token,
    );
    let session_key = format!("broker-control-{}", uuid::Uuid::new_v4());
    let (guide_tx, mut guide_rx) = tokio::sync::mpsc::channel(1);
    let (model_tx, mut model_rx) = tokio::sync::mpsc::channel(1);
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel();
    ACTIVE_WORKER_SESSIONS.insert(
        session_key.clone(),
        ExternalCliWorkerControlHandle {
            pid: 1,
            stop_tx,
            guide_tx,
            model_tx,
        },
    );
    let control = tokio::spawn(async move {
        let guide = guide_rx.recv().await.expect("brokered guide request");
        assert_eq!(guide.guide_id, "broker-guide");
        assert_eq!(guide.message, "steer the active turn");
        guide
            .ack_tx
            .send(ExternalCliGuideResult {
                guide_id: guide.guide_id,
                accepted: true,
                thread_id: Some("thread-from-main".to_string()),
                turn_id: Some("turn-from-main".to_string()),
                reason: None,
            })
            .expect("ack brokered guide");
        let model = model_rx.recv().await.expect("brokered model update");
        assert_eq!(model.model.as_deref(), Some("gpt-5.3-codex"));
        model
            .ack_tx
            .send(ExternalCliModelUpdateResult {
                update_id: model.update_id,
                model: model.model,
                accepted: true,
                thread_id: Some("thread-from-main".to_string()),
                reason: None,
            })
            .expect("ack brokered model update");
        let stop = stop_rx.recv().await.expect("brokered stop request");
        stop.send(()).expect("ack brokered stop");
    });

    let result = request_managed_session_guide(
        &session_key,
        "broker-guide".to_string(),
        "steer the active turn".to_string(),
    )
    .await
    .expect("guide through broker");
    assert!(result.accepted);
    assert_eq!(result.thread_id.as_deref(), Some("thread-from-main"));
    let model =
        request_managed_session_model_update(&session_key, Some("gpt-5.3-codex".to_string()))
            .await
            .expect("model update through broker");
    assert!(model.accepted);
    assert_eq!(model.model.as_deref(), Some("gpt-5.3-codex"));
    assert!(
        request_managed_session_stop(&default_runs_root(), &session_key,)
            .await
            .expect("stop through broker")
    );
    control.await.expect("join broker control receiver");

    let missing = format!("missing-broker-control-{}", uuid::Uuid::new_v4());
    let rejected = request_managed_session_guide(
        &missing,
        "missing-guide".to_string(),
        "cannot be delivered".to_string(),
    )
    .await
    .expect("missing main-process session is a runner rejection, not a broker failure");
    assert!(!rejected.accepted);
    assert!(
        rejected
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("no active external runner")),
        "{rejected:?}"
    );
    assert!(
        !request_managed_session_stop(&default_runs_root(), &missing,)
            .await
            .expect("missing stop should be a clean negative result")
    );
    assert!(request_managed_session_model_update(&missing, None)
        .await
        .unwrap_err()
        .contains("no active external runner"));

    drop(_addr);
    drop(_token);
    let _addr = EnvGuard::unset(crate::worker_runtime::im_broker::BROKER_ADDR_ENV);
    let _token = EnvGuard::unset(crate::worker_runtime::im_broker::BROKER_TOKEN_ENV);
    assert!(request_managed_session_guide(
        &missing,
        "unconfigured-guide".to_string(),
        "cannot be delivered".to_string(),
    )
    .await
    .unwrap_err()
    .contains("broker is not configured"));
    assert!(request_managed_session_model_update(&missing, None)
        .await
        .unwrap_err()
        .contains("broker is not configured"));
    assert_eq!(
        request_managed_session_model_update("", None)
            .await
            .unwrap_err(),
        "session_key cannot be empty"
    );
    assert!(request_managed_session_stop(&default_runs_root(), &missing)
        .await
        .unwrap_err()
        .contains("broker is not configured"));
    assert_eq!(
        request_managed_session_stop(&default_runs_root(), "")
            .await
            .unwrap_err(),
        "session_key cannot be empty"
    );
}

#[tokio::test]
async fn im_gateway_worker_run_requires_a_configured_main_broker() {
    let _registry_guard = external_cli_env_guard_async().await;
    let _worker = EnvGuard::set_str("BIFROST_IM_GATEWAY_WORKER", "1");
    let _addr = EnvGuard::unset(crate::worker_runtime::im_broker::BROKER_ADDR_ENV);
    let _token = EnvGuard::unset(crate::worker_runtime::im_broker::BROKER_TOKEN_ENV);
    let temp = tempfile::tempdir().unwrap();
    let runtime = ExternalCliRuntime::new(temp.path());

    let error = runtime
        .run_with_progress(worker_transport_test_request("missing-broker"), None)
        .await
        .unwrap_err();
    assert!(error.contains("broker is not configured"), "{error}");
}

#[tokio::test]
async fn managed_guide_and_model_update_use_the_local_registry_in_the_main_process() {
    let _registry_guard = external_cli_env_guard_async().await;
    let _worker = EnvGuard::unset("BIFROST_IM_GATEWAY_WORKER");
    let session_key = format!("local-managed-guide-{}", uuid::Uuid::new_v4());
    let (guide_tx, mut guide_rx) = tokio::sync::mpsc::channel(1);
    let (model_tx, mut model_rx) = tokio::sync::mpsc::channel(1);
    let (stop_tx, _stop_rx) = tokio::sync::mpsc::unbounded_channel();
    ACTIVE_WORKER_SESSIONS.insert(
        session_key.clone(),
        ExternalCliWorkerControlHandle {
            pid: 1,
            stop_tx,
            guide_tx,
            model_tx,
        },
    );
    let ack = tokio::spawn(async move {
        let guide = guide_rx.recv().await.expect("local guide request");
        guide
            .ack_tx
            .send(ExternalCliGuideResult {
                guide_id: guide.guide_id,
                accepted: true,
                thread_id: Some("local-thread".to_string()),
                turn_id: Some("local-turn".to_string()),
                reason: None,
            })
            .expect("ack local guide");
        let model = model_rx.recv().await.expect("local model update");
        model
            .ack_tx
            .send(ExternalCliModelUpdateResult {
                update_id: model.update_id,
                model: model.model,
                accepted: true,
                thread_id: Some("local-thread".to_string()),
                reason: None,
            })
            .expect("ack local model update");
    });

    let result = request_managed_session_guide(
        &session_key,
        "local-guide".to_string(),
        "guide locally".to_string(),
    )
    .await
    .expect("managed local guide");

    assert!(result.accepted);
    assert_eq!(result.thread_id.as_deref(), Some("local-thread"));
    let model = request_managed_session_model_update(&session_key, Some("gpt-local".to_string()))
        .await
        .expect("managed local model update");
    assert!(model.accepted);
    assert_eq!(model.model.as_deref(), Some("gpt-local"));
    ack.await.expect("join local guide acknowledgement");
    ACTIVE_WORKER_SESSIONS.remove(&session_key);
}

#[tokio::test]
async fn worker_model_update_validates_missing_closed_full_and_response_channels() {
    let _registry_guard = external_cli_env_guard_async().await;
    assert_eq!(
        request_worker_session_model_update(" ", None)
            .await
            .unwrap_err(),
        "session_key cannot be empty"
    );
    let missing = format!("missing-model-{}", uuid::Uuid::new_v4());
    assert!(request_worker_session_model_update(&missing, None)
        .await
        .unwrap_err()
        .contains("no active external runner"));

    let closed = format!("closed-model-{}", uuid::Uuid::new_v4());
    let (model_tx, model_rx) = tokio::sync::mpsc::channel(1);
    drop(model_rx);
    let (guide_tx, _guide_rx) = tokio::sync::mpsc::channel(1);
    let (stop_tx, _stop_rx) = tokio::sync::mpsc::unbounded_channel();
    ACTIVE_WORKER_SESSIONS.insert(
        closed.clone(),
        ExternalCliWorkerControlHandle {
            pid: 1,
            stop_tx,
            guide_tx,
            model_tx,
        },
    );
    assert!(request_worker_session_model_update(&closed, None)
        .await
        .unwrap_err()
        .contains("control channel closed"));
    ACTIVE_WORKER_SESSIONS.remove(&closed);

    let full = format!("full-model-{}", uuid::Uuid::new_v4());
    let (model_tx, mut model_rx) = tokio::sync::mpsc::channel(1);
    let (guide_tx, _guide_rx) = tokio::sync::mpsc::channel(1);
    let (stop_tx, _stop_rx) = tokio::sync::mpsc::unbounded_channel();
    ACTIVE_WORKER_SESSIONS.insert(
        full.clone(),
        ExternalCliWorkerControlHandle {
            pid: 1,
            stop_tx,
            guide_tx,
            model_tx,
        },
    );
    let first_session = full.clone();
    let first = tokio::spawn(async move {
        request_worker_session_model_update(&first_session, Some("first".to_string())).await
    });
    timeout(Duration::from_secs(1), async {
        while model_rx.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first model request queued");
    assert!(
        request_worker_session_model_update(&full, Some("second".to_string()))
            .await
            .unwrap_err()
            .contains("too many pending model updates")
    );
    drop(model_rx.recv().await.expect("queued model request"));
    assert!(first
        .await
        .unwrap()
        .unwrap_err()
        .contains("response closed"));
    ACTIVE_WORKER_SESSIONS.remove(&full);
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn worker_transport_test_request(session_key: &str) -> ExternalCliRunRequest {
    ExternalCliRunRequest {
        message: "worker transport test".to_string(),
        images: Vec::new(),
        files: Vec::new(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("worker-transport-test".to_string()),
        runner_id: Some("worker-transport-test".to_string()),
        session_key: Some(session_key.to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    }
}

fn delayed_final_command(content: &str) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        (
            "cmd.exe".to_string(),
            vec![
                "/C".to_string(),
                format!(
                    "ping -n 3 127.0.0.1 >nul & echo {{\"type\":\"assistant_final\",\"content\":\"{content}\"}}"
                ),
            ],
        )
    }
    #[cfg(not(windows))]
    {
        (
            "sh".to_string(),
            vec![
                "-c".to_string(),
                format!(
                    "sleep 2; printf '%s\\n' '{{\"type\":\"assistant_final\",\"content\":\"{content}\"}}'"
                ),
            ],
        )
    }
}

#[cfg(unix)]
#[test]
fn terminate_process_group_force_kills_sigterm_ignoring_process() {
    use std::os::unix::process::CommandExt;
    use std::process::Command as StdProcessCommand;
    use std::time::Instant;

    let mut child = StdProcessCommand::new("sh")
        .arg("-c")
        .arg("trap '' TERM; while true; do sleep 1; done")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn SIGTERM-ignoring process");
    let pid = child.id();

    terminate_process_group(pid).expect("terminate process group");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                panic!("process ignored SIGTERM and was not force-killed");
            }
            Err(error) => panic!("wait SIGTERM-ignoring process: {error}"),
        }
    }
}

#[cfg(unix)]
#[test]
fn signal_process_group_reports_not_found_after_child_exits() {
    use std::os::unix::process::CommandExt;
    use std::process::Command as StdProcessCommand;

    let mut child = StdProcessCommand::new("sh")
        .arg("-c")
        .arg("exit 0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn short-lived process");
    let pid = child.id();
    child.wait().expect("wait short-lived process");

    assert_eq!(
        signal_process_group_or_child(pid, nix::sys::signal::Signal::SIGTERM)
            .expect("signal missing process group"),
        ProcessSignalOutcome::NotFound
    );
    terminate_process_group(pid).expect("terminate missing process group is a no-op");
}

#[cfg(unix)]
#[tokio::test]
async fn stale_external_worker_entry_does_not_kill_pid_when_stop_receiver_is_gone() {
    let _registry_guard = external_cli_env_guard_async().await;
    use std::os::unix::process::CommandExt;
    use std::process::Command as StdProcessCommand;

    let mut child = StdProcessCommand::new("sh")
        .arg("-c")
        .arg("sleep 5")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn protected external worker process");
    let pid = child.id();
    let (stop_tx, stop_rx) = tokio::sync::mpsc::unbounded_channel();
    drop(stop_rx);
    let (guide_tx, _guide_rx) = tokio::sync::mpsc::channel(1);
    ACTIVE_WORKER_SESSIONS.insert(
        "stale-external-worker".to_string(),
        ExternalCliWorkerControlHandle {
            pid,
            stop_tx,
            guide_tx,
            model_tx: unused_model_tx(),
        },
    );

    assert!(!request_worker_session_stop("stale-external-worker").await);
    assert!(
        child
            .try_wait()
            .expect("poll protected external worker process")
            .is_none(),
        "stale external worker registry entry must not terminate a pid when stop receiver is gone"
    );
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
#[tokio::test]
async fn acknowledged_external_worker_stop_does_not_kill_pid() {
    let _registry_guard = external_cli_env_guard_async().await;
    use std::os::unix::process::CommandExt;
    use std::process::Command as StdProcessCommand;

    let mut child = StdProcessCommand::new("sh")
        .arg("-c")
        .arg("sleep 5")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn protected external worker process");
    let pid = child.id();
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel();
    let (guide_tx, _guide_rx) = tokio::sync::mpsc::channel(1);
    ACTIVE_WORKER_SESSIONS.insert(
        "acked-external-worker".to_string(),
        ExternalCliWorkerControlHandle {
            pid,
            stop_tx,
            guide_tx,
            model_tx: unused_model_tx(),
        },
    );
    let ack_task = tokio::spawn(async move {
        if let Some(ack_tx) = stop_rx.recv().await {
            let _ = ack_tx.send(());
        }
    });

    assert!(request_worker_session_stop("acked-external-worker").await);
    ack_task.await.expect("ack task");
    assert!(
        child
            .try_wait()
            .expect("poll protected external worker process")
            .is_none(),
        "acknowledged external worker stop must not be followed by pid termination"
    );
    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test]
async fn worker_stop_rejects_empty_and_missing_session_keys() {
    let _registry_guard = external_cli_env_guard_async().await;
    assert!(!request_worker_session_stop("   ").await);
    assert!(!request_worker_session_stop("missing-external-worker").await);
}

#[cfg(unix)]
#[tokio::test]
async fn unacknowledged_external_worker_stop_uses_owned_pid_fallback() {
    let _registry_guard = external_cli_env_guard_async().await;
    use std::os::unix::process::CommandExt;
    use std::process::Command as StdProcessCommand;

    let mut child = StdProcessCommand::new("sh")
        .arg("-c")
        .arg("sleep 30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn unacknowledged external worker process");
    let pid = child.id();
    let session_key = format!("unacknowledged-worker-{}", uuid::Uuid::new_v4());
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel();
    let (guide_tx, _guide_rx) = tokio::sync::mpsc::channel(1);
    ACTIVE_WORKERS.insert(pid, stop_tx.clone());
    ACTIVE_WORKER_SESSIONS.insert(
        session_key.clone(),
        ExternalCliWorkerControlHandle {
            pid,
            stop_tx: stop_tx.clone(),
            guide_tx,
            model_tx: unused_model_tx(),
        },
    );
    let consume_without_ack = tokio::spawn(async move {
        let _ack_tx = stop_rx.recv().await.expect("stop request");
        tokio::time::sleep(Duration::from_secs(10)).await;
    });

    assert!(request_worker_session_stop(&session_key).await);
    let status = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(status) = child.try_wait().expect("poll fallback-killed worker") {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("owned worker pid should be terminated after grace period");
    assert!(!status.success());
    consume_without_ack.abort();
    ACTIVE_WORKERS.remove(&pid);
}

#[tokio::test]
async fn sessionless_registry_job_can_cancel_while_queued() {
    let _registry_guard = external_cli_env_guard_async().await;
    let registry_id = format!("sessionless-{}", uuid::Uuid::new_v4());
    let queue_id = uuid::Uuid::new_v4().to_string();
    let (cancel_tx, cancel_rx) = watch::channel(false);
    QUEUED_WORKER_SESSIONS.insert(
        registry_id.clone(),
        QueuedExternalCliWorkerControl {
            queue_id,
            cancel_tx,
        },
    );

    assert!(request_worker_session_stop(&registry_id).await);
    assert!(*cancel_rx.borrow());
    QUEUED_WORKER_SESSIONS.remove(&registry_id);
}

#[tokio::test]
async fn registered_worker_stop_resolves_named_and_sessionless_runs() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let runs_root = temp_dir.path();

    let named_run = format!("named-run-{}", uuid::Uuid::new_v4());
    let named_session = format!("named-session-{}", uuid::Uuid::new_v4());
    tokio::fs::create_dir_all(runs_root.join(&named_run))
        .await
        .unwrap();
    ACTIVE_RUNS.insert(named_run.clone(), 999_999_999);
    ACTIVE_SESSIONS.insert(named_session.clone(), named_run.clone());
    assert!(request_registered_worker_run_stop(runs_root, &named_session).await);
    assert!(runs_root.join(&named_run).join("stop_requested").is_file());
    ACTIVE_RUNS.remove(&named_run);
    ACTIVE_SESSIONS.remove(&named_session);

    let sessionless_run = format!("sessionless-run-{}", uuid::Uuid::new_v4());
    tokio::fs::create_dir_all(runs_root.join(&sessionless_run))
        .await
        .unwrap();
    ACTIVE_RUNS.insert(sessionless_run.clone(), 999_999_998);
    assert!(request_registered_worker_run_stop(runs_root, "").await);
    assert!(runs_root
        .join(&sessionless_run)
        .join("stop_requested")
        .is_file());
    ACTIVE_RUNS.remove(&sessionless_run);
}

fn worker_exec_request(session_key: Option<String>, script: &str) -> ExternalCliRunRequest {
    ExternalCliRunRequest {
        message: "worker request".to_string(),
        images: Vec::new(),
        files: Vec::new(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("worker-test-provider".to_string()),
        runner_id: Some("worker-test-runner".to_string()),
        session_key,
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "mock".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("sh".to_string()),
            args: vec!["-c".to_string(), script.to_string()],
            transport: Some(ExternalCliTransport::Exec),
            timeout_secs: Some(30),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    }
}

#[test]
fn worker_stdio_subprocess_entrypoint() {
    if std::env::var_os("BIFROST_TEST_REAL_WORKER_CHILD").is_some() {
        let marker = std::env::var_os("BIFROST_TEST_REAL_WORKER_MARKER")
            .expect("real worker child marker path");
        std::fs::write(marker, b"entered").expect("write real worker child marker");
        run_worker_stdio().expect("run real external worker stdio loop");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn real_worker_stdio_subprocess_interrupts_via_protocol() {
    let _registry_guard = external_cli_env_guard_async().await;
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().unwrap();
    let executable = temp_dir.path().join("real-worker-test.py");
    std::fs::write(
        &executable,
        r#"#!/usr/bin/env python3
import os
import subprocess
import sys

test_exe = os.environ["BIFROST_TEST_REAL_WORKER_EXE"]
child = subprocess.Popen(
    [test_exe, "--exact", "im_gateway::external_cli::tests::worker_stdio_subprocess_entrypoint", "--nocapture", "--quiet"],
    stdin=sys.stdin,
    stdout=subprocess.PIPE,
    stderr=sys.stderr,
    text=True,
)
for line in child.stdout:
    if line.lstrip().startswith("{"):
        sys.stdout.write(line)
        sys.stdout.flush()
sys.exit(child.wait())
"#,
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let current_exe = std::env::current_exe().unwrap();
    let marker = temp_dir.path().join("worker-entered");
    let _child_mode = EnvGuard::set("BIFROST_TEST_REAL_WORKER_CHILD", Path::new("1"));
    let _child_exe = EnvGuard::set("BIFROST_TEST_REAL_WORKER_EXE", &current_exe);
    let _child_marker = EnvGuard::set("BIFROST_TEST_REAL_WORKER_MARKER", &marker);
    let _env_bootstrap = EnvGuard::set(
        TEST_EXTERNAL_CLI_WORKER_FORCE_ENV_BOOTSTRAP_ENV,
        Path::new("1"),
    );
    let _force = EnvGuard::set("BIFROST_FORCE_EXTERNAL_CLI_WORKER", Path::new("1"));
    let _executable = EnvGuard::set(TEST_EXTERNAL_CLI_WORKER_EXECUTABLE_ENV, &executable);
    let session_key = format!("real-worker-stdio-{}", uuid::Uuid::new_v4());
    let runtime = ExternalCliRuntime::new(temp_dir.path().join("runs"));
    let request = worker_exec_request(Some(session_key.clone()), "sleep 30");
    let task = tokio::spawn(async move { runtime.run(request).await });
    wait_for_parent_worker_handle(&session_key).await;
    timeout(Duration::from_secs(5), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("real worker subprocess entrypoint");

    assert!(request_worker_session_stop(&session_key).await);
    let result = timeout(Duration::from_secs(8), task)
        .await
        .expect("real worker stdio completion")
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read(&marker).unwrap(), b"entered");
    assert_eq!(result.status, ExternalCliRunStatus::Stopped);
}

async fn collect_worker_events(
    events: &mut mpsc::Receiver<ExternalCliWorkerEvent>,
) -> Vec<ExternalCliWorkerEvent> {
    let mut collected = Vec::new();
    while let Some(event) = events.recv().await {
        collected.push(event);
    }
    collected
}

#[cfg(unix)]
fn fake_parent_worker(temp_dir: &tempfile::TempDir) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = temp_dir.path().join("fake-external-worker.py");
    std::fs::write(
        &path,
        r#"#!/usr/bin/env python3
import json
import os
import subprocess
import sys
import time

scenario = os.environ.get("BIFROST_TEST_EXTERNAL_CLI_WORKER_SCENARIO", "stopped")

def emit(value):
    sys.stdout.write(json.dumps(value, separators=(",", ":")) + "\n")
    sys.stdout.flush()

run = sys.stdin.readline()
if not run:
    sys.exit(2)
emit({"type":"started","sessionKey":None,"pid":os.getpid()})

if scenario == "close_stdin":
    os.close(0)
    time.sleep(10)
elif scenario == "closed_stdin_with_stdout_holder":
    subprocess.Popen(["sleep", "10"], stdin=subprocess.DEVNULL, stdout=sys.stdout, stderr=sys.stderr)
    os._exit(0)
elif scenario == "delayed_stopped":
    time.sleep(0.2)
    emit({"type":"stopped"})
elif scenario == "delayed_failed":
    time.sleep(0.1)
    emit({"type":"failed","error":"fake parent worker failure"})
elif scenario == "delayed_eof":
    time.sleep(0.1)
elif scenario == "finished":
    emit({"type":"heartbeat","timestamp_ms":1})
    emit({"type":"progress","event":{
        "eventType":"status","content":"fake progress","title":None,"raw":{}
    }})
    emit({"type":"finished","result":{"resultPath":os.environ["BIFROST_TEST_WORKER_RESULT_PATH"]}})
else:
    for raw in sys.stdin:
        command = json.loads(raw)
        kind = command.get("type")
        if kind == "guide":
            guide_id = command["guide_id"]
            if guide_id == "accepted":
                emit({"type":"guide_result","result":{
                    "guideId":guide_id,
                    "accepted":True,
                    "threadId":"thread-test",
                    "turnId":"turn-test"
                }})
        elif kind == "model_update":
            update_id = command["update_id"]
            if update_id == "accepted-model":
                emit({"type":"model_update_result","result":{
                    "updateId":update_id,
                    "model":command.get("model"),
                    "accepted":True,
                    "threadId":"thread-test"
                }})
        elif kind == "stop":
            if scenario == "failed_on_stop":
                time.sleep(0.2)
                emit({"type":"failed","error":"fake failure while stopping"})
            elif scenario == "invalid_finished_on_stop":
                emit({"type":"finished","result":{"resultPath":os.environ["BIFROST_TEST_INVALID_WORKER_RESULT_PATH"]}})
            elif scenario == "eof_on_stop":
                pass
            elif scenario == "hang_on_stop":
                time.sleep(10)
            else:
                emit({"type":"stopped"})
            break
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

#[cfg(unix)]
async fn wait_for_parent_worker_handle(session_key: &str) -> ExternalCliWorkerControlHandle {
    timeout(Duration::from_secs(5), async {
        loop {
            if let Some(handle) = ACTIVE_WORKER_SESSIONS.get(session_key) {
                break handle.value().clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("parent worker registration")
}

#[cfg(unix)]
async fn spawn_fake_parent_worker(
    temp_dir: &tempfile::TempDir,
    scenario: &'static str,
) -> (
    String,
    tokio::task::JoinHandle<Result<ExternalCliRunResult, String>>,
    EnvGuard,
    EnvGuard,
    EnvGuard,
) {
    let executable = fake_parent_worker(temp_dir);
    let force_worker = EnvGuard::set("BIFROST_FORCE_EXTERNAL_CLI_WORKER", Path::new("1"));
    let executable_guard = EnvGuard::set(TEST_EXTERNAL_CLI_WORKER_EXECUTABLE_ENV, &executable);
    let scenario_guard = EnvGuard::set(
        "BIFROST_TEST_EXTERNAL_CLI_WORKER_SCENARIO",
        Path::new(scenario),
    );
    let session_key = format!("fake-parent-worker-{}", uuid::Uuid::new_v4());
    let runtime = ExternalCliRuntime::new(temp_dir.path().join("runs"));
    let request = worker_exec_request(Some(session_key.clone()), "sleep 30");
    let task = tokio::spawn(async move { runtime.run(request).await });
    (
        session_key,
        task,
        force_worker,
        executable_guard,
        scenario_guard,
    )
}

#[cfg(unix)]
#[tokio::test]
async fn parent_worker_stop_write_failure_terminates_and_acknowledges() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let _stop_failure = EnvGuard::set(
        TEST_EXTERNAL_CLI_PARENT_STOP_WRITE_FAILURE_ENV,
        Path::new("1"),
    );
    let (session_key, task, _force, _executable, _scenario) =
        spawn_fake_parent_worker(&temp_dir, "close_stdin").await;
    let handle = wait_for_parent_worker_handle(&session_key).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (ack_tx, ack_rx) = oneshot::channel();
    handle.stop_tx.send(ack_tx).unwrap();

    timeout(Duration::from_secs(8), ack_rx)
        .await
        .expect("bounded stop acknowledgement")
        .unwrap();
    let result = timeout(Duration::from_secs(8), task)
        .await
        .expect("bounded parent completion")
        .unwrap()
        .unwrap();
    assert_eq!(result.status, ExternalCliRunStatus::Stopped);
}

#[cfg(unix)]
#[tokio::test]
async fn parent_worker_acks_duplicate_stop_rejects_guide_and_accepts_failed_exit() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let (session_key, task, _force, _executable, _scenario) =
        spawn_fake_parent_worker(&temp_dir, "failed_on_stop").await;
    let handle = wait_for_parent_worker_handle(&session_key).await;
    let (first_ack_tx, first_ack_rx) = oneshot::channel();
    handle.stop_tx.send(first_ack_tx).unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    let (second_ack_tx, second_ack_rx) = oneshot::channel();
    handle.stop_tx.send(second_ack_tx).unwrap();
    let (guide_ack_tx, guide_ack_rx) = oneshot::channel();
    handle
        .guide_tx
        .send(ExternalCliWorkerGuideRequest {
            guide_id: "during-stop".to_string(),
            message: "ignored".to_string(),
            ack_tx: guide_ack_tx,
        })
        .await
        .unwrap();
    let (model_ack_tx, model_ack_rx) = oneshot::channel();
    handle
        .model_tx
        .send(ExternalCliWorkerModelUpdateRequest {
            update_id: "during-stop-model".to_string(),
            model: Some("gpt-test".to_string()),
            ack_tx: model_ack_tx,
        })
        .await
        .unwrap();

    let rejected = guide_ack_rx.await.unwrap();
    assert_eq!(
        rejected.reason.as_deref(),
        Some("external runner is stopping")
    );
    assert_eq!(
        model_ack_rx.await.unwrap().reason.as_deref(),
        Some("external runner is stopping")
    );
    first_ack_rx.await.unwrap();
    second_ack_rx.await.unwrap();
    assert_eq!(
        task.await.unwrap().unwrap().status,
        ExternalCliRunStatus::Stopped
    );
}

#[cfg(unix)]
#[tokio::test]
async fn parent_worker_treats_event_eof_during_stop_as_stopped() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let (session_key, task, _force, _executable, _scenario) =
        spawn_fake_parent_worker(&temp_dir, "eof_on_stop").await;
    let handle = wait_for_parent_worker_handle(&session_key).await;
    let (ack_tx, ack_rx) = oneshot::channel();
    handle.stop_tx.send(ack_tx).unwrap();

    ack_rx.await.unwrap();
    assert_eq!(
        task.await.unwrap().unwrap().status,
        ExternalCliRunStatus::Stopped
    );
}

#[cfg(unix)]
#[tokio::test]
async fn parent_worker_invalid_result_path_still_cleans_pending_control_requests() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let data_dir = temp_dir.path().join("data");
    let _data_dir = ExternalCliDataDirOverride::set(&data_dir);
    std::fs::create_dir_all(external_cli_worker_result_dir()).unwrap();
    let outside_result = temp_dir.path().join("outside-worker-results.json");
    std::fs::write(&outside_result, b"{}").unwrap();
    let _outside_result = EnvGuard::set("BIFROST_TEST_INVALID_WORKER_RESULT_PATH", &outside_result);
    let (session_key, task, _force, _executable, _scenario) =
        spawn_fake_parent_worker(&temp_dir, "invalid_finished_on_stop").await;
    let handle = wait_for_parent_worker_handle(&session_key).await;
    let (guide_tx, guide_rx) = oneshot::channel();
    handle
        .guide_tx
        .send(ExternalCliWorkerGuideRequest {
            guide_id: "pending-at-invalid-finish".to_string(),
            message: "pending".to_string(),
            ack_tx: guide_tx,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (stop_tx, stop_rx) = oneshot::channel();
    handle.stop_tx.send(stop_tx).unwrap();

    stop_rx.await.unwrap();
    assert_eq!(
        guide_rx.await.unwrap().reason.as_deref(),
        Some("external runner finished before guide acknowledgement")
    );
    let error = task.await.unwrap().unwrap_err();
    assert!(error.contains("outside"), "unexpected error: {error}");
}

#[cfg(unix)]
#[tokio::test]
async fn parent_worker_timeout_terminates_unresponsive_worker() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let (session_key, task, _force, _executable, _scenario) =
        spawn_fake_parent_worker(&temp_dir, "hang_on_stop").await;
    let handle = wait_for_parent_worker_handle(&session_key).await;
    let (ack_tx, ack_rx) = oneshot::channel();
    handle.stop_tx.send(ack_tx).unwrap();

    timeout(Duration::from_secs(8), ack_rx)
        .await
        .expect("hard-stop acknowledgement")
        .unwrap();
    assert_eq!(
        timeout(Duration::from_secs(8), task)
            .await
            .expect("hard-stop completion")
            .unwrap()
            .unwrap()
            .status,
        ExternalCliRunStatus::Stopped
    );
}

#[cfg(unix)]
#[tokio::test]
async fn parent_worker_routes_guides_bounds_pending_and_rejects_them_on_finish() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let (session_key, task, _force, _executable, _scenario) =
        spawn_fake_parent_worker(&temp_dir, "stopped").await;
    let handle = wait_for_parent_worker_handle(&session_key).await;

    let (accepted_tx, accepted_rx) = oneshot::channel();
    handle
        .guide_tx
        .send(ExternalCliWorkerGuideRequest {
            guide_id: "accepted".to_string(),
            message: "accepted".to_string(),
            ack_tx: accepted_tx,
        })
        .await
        .unwrap();
    let accepted = accepted_rx.await.unwrap();
    assert!(accepted.accepted, "unexpected guide result: {accepted:?}");

    let (first_tx, first_rx) = oneshot::channel();
    handle
        .guide_tx
        .send(ExternalCliWorkerGuideRequest {
            guide_id: "duplicate".to_string(),
            message: "first".to_string(),
            ack_tx: first_tx,
        })
        .await
        .unwrap();
    let (duplicate_tx, duplicate_rx) = oneshot::channel();
    handle
        .guide_tx
        .send(ExternalCliWorkerGuideRequest {
            guide_id: "duplicate".to_string(),
            message: "second".to_string(),
            ack_tx: duplicate_tx,
        })
        .await
        .unwrap();
    assert_eq!(
        duplicate_rx.await.unwrap().reason.as_deref(),
        Some("duplicate guide id is already pending")
    );

    let mut pending = vec![first_rx];
    for index in 1..MAX_PENDING_EXTERNAL_GUIDES {
        let (ack_tx, ack_rx) = oneshot::channel();
        handle
            .guide_tx
            .send(ExternalCliWorkerGuideRequest {
                guide_id: format!("pending-{index}"),
                message: "pending".to_string(),
                ack_tx,
            })
            .await
            .unwrap();
        pending.push(ack_rx);
    }
    let (overflow_tx, overflow_rx) = oneshot::channel();
    handle
        .guide_tx
        .send(ExternalCliWorkerGuideRequest {
            guide_id: "overflow".to_string(),
            message: "overflow".to_string(),
            ack_tx: overflow_tx,
        })
        .await
        .unwrap();
    assert!(overflow_rx
        .await
        .unwrap()
        .reason
        .unwrap()
        .contains("too many pending guide requests"));

    let (stop_tx, stop_rx) = oneshot::channel();
    handle.stop_tx.send(stop_tx).unwrap();
    stop_rx.await.unwrap();
    assert_eq!(
        task.await.unwrap().unwrap().status,
        ExternalCliRunStatus::Stopped
    );
    for ack_rx in pending {
        assert_eq!(
            ack_rx.await.unwrap().reason.as_deref(),
            Some("external runner finished before guide acknowledgement")
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn parent_worker_routes_model_updates_bounds_pending_and_rejects_them_on_finish() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let (session_key, task, _force, _executable, _scenario) =
        spawn_fake_parent_worker(&temp_dir, "stopped").await;
    let handle = wait_for_parent_worker_handle(&session_key).await;

    let (accepted_tx, accepted_rx) = oneshot::channel();
    handle
        .model_tx
        .send(ExternalCliWorkerModelUpdateRequest {
            update_id: "accepted-model".to_string(),
            model: Some("gpt-accepted".to_string()),
            ack_tx: accepted_tx,
        })
        .await
        .unwrap();
    let accepted = accepted_rx.await.unwrap();
    assert!(accepted.accepted, "unexpected model result: {accepted:?}");
    assert_eq!(accepted.model.as_deref(), Some("gpt-accepted"));

    let (first_tx, first_rx) = oneshot::channel();
    handle
        .model_tx
        .send(ExternalCliWorkerModelUpdateRequest {
            update_id: "pending-model".to_string(),
            model: Some("gpt-pending".to_string()),
            ack_tx: first_tx,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    let (duplicate_tx, duplicate_rx) = oneshot::channel();
    handle
        .model_tx
        .send(ExternalCliWorkerModelUpdateRequest {
            update_id: "pending-model".to_string(),
            model: Some("gpt-duplicate".to_string()),
            ack_tx: duplicate_tx,
        })
        .await
        .unwrap();
    assert_eq!(
        duplicate_rx.await.unwrap().reason.as_deref(),
        Some("duplicate model update id is already pending")
    );

    let mut pending = vec![first_rx];
    for index in 1..MAX_PENDING_EXTERNAL_GUIDES {
        let (ack_tx, ack_rx) = oneshot::channel();
        handle
            .model_tx
            .send(ExternalCliWorkerModelUpdateRequest {
                update_id: format!("pending-model-{index}"),
                model: None,
                ack_tx,
            })
            .await
            .unwrap();
        pending.push(ack_rx);
    }
    tokio::time::sleep(Duration::from_millis(30)).await;
    let (overflow_tx, overflow_rx) = oneshot::channel();
    handle
        .model_tx
        .send(ExternalCliWorkerModelUpdateRequest {
            update_id: "overflow-model".to_string(),
            model: None,
            ack_tx: overflow_tx,
        })
        .await
        .unwrap();
    assert!(overflow_rx
        .await
        .unwrap()
        .reason
        .unwrap()
        .contains("too many pending model updates"));

    let (stop_tx, stop_rx) = oneshot::channel();
    handle.stop_tx.send(stop_tx).unwrap();
    stop_rx.await.unwrap();
    assert_eq!(
        task.await.unwrap().unwrap().status,
        ExternalCliRunStatus::Stopped
    );
    for ack_rx in pending {
        let rejected = ack_rx.await.unwrap();
        assert_eq!(
            rejected.reason.as_deref(),
            Some("external runner finished before model update acknowledgement")
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn parent_worker_guide_channel_closure_does_not_spin() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let (session_key, task, _force, _executable, _scenario) =
        spawn_fake_parent_worker(&temp_dir, "delayed_stopped").await;
    let handle = wait_for_parent_worker_handle(&session_key).await;
    ACTIVE_WORKER_SESSIONS.remove(&session_key);
    drop(handle);

    assert_eq!(
        timeout(Duration::from_secs(3), task)
            .await
            .expect("parent exits after guide channel closes")
            .unwrap()
            .unwrap()
            .status,
        ExternalCliRunStatus::Stopped
    );
}

#[cfg(unix)]
#[tokio::test]
async fn parent_worker_sessionless_finish_routes_progress_and_reads_result() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let executable = fake_parent_worker(&temp_dir);
    let result_path =
        external_cli_worker_result_dir().join(format!("result-{}.json", uuid::Uuid::new_v4()));
    let expected = ExternalCliRunResult::stopped(None, "mock".to_string());
    write_external_cli_worker_json(
        &result_path,
        &expected,
        EXTERNAL_CLI_WORKER_RESULT_MAX_BYTES,
    )
    .unwrap();
    let _force = EnvGuard::set("BIFROST_FORCE_EXTERNAL_CLI_WORKER", Path::new("1"));
    let _executable = EnvGuard::set(TEST_EXTERNAL_CLI_WORKER_EXECUTABLE_ENV, &executable);
    let _scenario = EnvGuard::set(
        "BIFROST_TEST_EXTERNAL_CLI_WORKER_SCENARIO",
        Path::new("finished"),
    );
    let _result_path = EnvGuard::set("BIFROST_TEST_WORKER_RESULT_PATH", &result_path);
    let runtime = ExternalCliRuntime::new(temp_dir.path().join("runs"));
    let (progress_tx, mut progress_rx) = mpsc::channel(4);

    let result = runtime
        .run_with_progress(worker_exec_request(None, "sleep 30"), Some(progress_tx))
        .await
        .unwrap();

    assert_eq!(result.status, ExternalCliRunStatus::Stopped);
    assert_eq!(progress_rx.recv().await.unwrap().content, "fake progress");
    assert!(!result_path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn parent_worker_reports_normal_failure_and_eof() {
    let _registry_guard = external_cli_env_guard_async().await;
    for (scenario, expected) in [
        ("delayed_failed", "fake parent worker failure"),
        ("delayed_eof", "exited before final event"),
    ] {
        let temp_dir = tempfile::tempdir().unwrap();
        let (_session_key, task, _force, _executable, _scenario) =
            spawn_fake_parent_worker(&temp_dir, scenario).await;
        let error = task.await.unwrap().unwrap_err();
        assert!(
            error.contains(expected),
            "unexpected {scenario} error: {error}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn parent_worker_rejects_oversized_guide_before_worker_write() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let (session_key, task, _force, _executable, _scenario) =
        spawn_fake_parent_worker(&temp_dir, "stopped").await;
    let handle = wait_for_parent_worker_handle(&session_key).await;
    let (guide_tx, guide_rx) = oneshot::channel();
    handle
        .guide_tx
        .send(ExternalCliWorkerGuideRequest {
            guide_id: "closed-stdin".to_string(),
            message: "x".repeat(EXTERNAL_CLI_WORKER_MAX_FRAME_BYTES),
            ack_tx: guide_tx,
        })
        .await
        .unwrap();
    let rejected = guide_rx.await.unwrap();
    assert!(!rejected.accepted);
    let reason = rejected.reason.unwrap();
    assert!(
        reason.contains("exceeds hard limit"),
        "unexpected reason: {reason}"
    );
    let (stop_tx, stop_rx) = oneshot::channel();
    handle.stop_tx.send(stop_tx).unwrap();
    stop_rx.await.unwrap();
    assert_eq!(
        task.await.unwrap().unwrap().status,
        ExternalCliRunStatus::Stopped
    );
}

#[cfg(unix)]
#[tokio::test]
async fn worker_request_handles_guide_model_update_duplicate_stop_and_command_eof() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let session_key = format!("worker-loop-stop-{}", uuid::Uuid::new_v4());
    let request = worker_exec_request(Some(session_key.clone()), "cat >/dev/null; sleep 30");
    let (command_tx, command_rx) = mpsc::channel(MAX_PENDING_EXTERNAL_GUIDES + 2);
    let (event_tx, mut event_rx) = mpsc::channel(128);
    let worker = tokio::spawn(run_worker_request(
        temp_dir.path().to_path_buf(),
        request.clone(),
        command_rx,
        event_tx,
    ));
    timeout(Duration::from_secs(5), async {
        while !ACTIVE_SESSIONS.contains_key(&session_key) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("worker run registration");

    command_tx
        .send(ExternalCliWorkerCommand::Guide {
            guide_id: "exec-guide".to_string(),
            message: "cannot steer exec".to_string(),
        })
        .await
        .unwrap();
    command_tx
        .send(ExternalCliWorkerCommand::Run {
            request: Box::new(ExternalCliWorkerRunRequest {
                protocol_version: WORKER_PROTOCOL_VERSION,
                runs_root: temp_dir.path().display().to_string(),
                request_path: temp_dir.path().join("ignored.json"),
            }),
        })
        .await
        .unwrap();
    command_tx
        .send(ExternalCliWorkerCommand::ModelUpdate {
            update_id: "exec-model".to_string(),
            model: Some("unsupported-on-exec".to_string()),
        })
        .await
        .unwrap();
    command_tx
        .send(ExternalCliWorkerCommand::Stop)
        .await
        .unwrap();
    command_tx
        .send(ExternalCliWorkerCommand::Stop)
        .await
        .unwrap();
    drop(command_tx);

    timeout(Duration::from_secs(8), worker)
        .await
        .expect("worker loop stop completion")
        .unwrap()
        .unwrap();
    let events = collect_worker_events(&mut event_rx).await;
    assert!(events.iter().any(|event| matches!(
        event,
        ExternalCliWorkerEvent::GuideResult { result }
            if result.guide_id == "exec-guide" && !result.accepted
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ExternalCliWorkerEvent::ModelUpdateResult { result }
            if result.update_id == "exec-model" && !result.accepted
                && result.reason.as_deref().is_some_and(|reason| reason.contains("exec transport"))
    )));
    for event in events {
        if let ExternalCliWorkerEvent::Finished { result } = event {
            let _ = std::fs::remove_file(result.result_path);
        }
    }
}

#[tokio::test]
async fn worker_request_eof_before_run_registration_stops_cleanly() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let request = worker_exec_request(None, "sleep 1; exit 7");
    let (command_tx, command_rx) = mpsc::channel(MAX_PENDING_EXTERNAL_GUIDES + 2);
    let (event_tx, mut event_rx) = mpsc::channel(128);
    drop(command_tx);

    run_worker_request(temp_dir.path().to_path_buf(), request, command_rx, event_tx)
        .await
        .unwrap();

    let events = collect_worker_events(&mut event_rx).await;
    assert!(matches!(
        events.last(),
        Some(ExternalCliWorkerEvent::Stopped | ExternalCliWorkerEvent::Finished { .. })
    ));
    for event in events {
        if let ExternalCliWorkerEvent::Finished { result } = event {
            let _ = std::fs::remove_file(result.result_path);
        }
    }
}

#[tokio::test]
async fn worker_request_aborts_when_native_stop_cannot_be_registered() {
    let _registry_guard = external_cli_env_guard_async().await;
    let _stop_outcome = EnvGuard::set(
        TEST_EXTERNAL_CLI_WORKER_STOP_OUTCOME_ENV,
        Path::new("rejected"),
    );
    let temp_dir = tempfile::tempdir().unwrap();
    let request = worker_exec_request(None, "sleep 30");
    let (command_tx, command_rx) = mpsc::channel(1);
    let (event_tx, mut event_rx) = mpsc::channel(8);
    command_tx
        .send(ExternalCliWorkerCommand::Stop)
        .await
        .unwrap();
    drop(command_tx);

    timeout(
        Duration::from_secs(3),
        run_worker_request(temp_dir.path().to_path_buf(), request, command_rx, event_tx),
    )
    .await
    .expect("rejected native stop must abort the worker task")
    .unwrap();
    assert!(collect_worker_events(&mut event_rx)
        .await
        .iter()
        .any(|event| matches!(event, ExternalCliWorkerEvent::Stopped)));
}

#[tokio::test]
async fn worker_request_hard_stops_after_native_stop_grace_period() {
    let _registry_guard = external_cli_env_guard_async().await;
    let _stop_outcome = EnvGuard::set(
        TEST_EXTERNAL_CLI_WORKER_STOP_OUTCOME_ENV,
        Path::new("accepted"),
    );
    let temp_dir = tempfile::tempdir().unwrap();
    let request = worker_exec_request(None, "sleep 30");
    let (command_tx, command_rx) = mpsc::channel(1);
    let (event_tx, mut event_rx) = mpsc::channel(8);
    command_tx
        .send(ExternalCliWorkerCommand::Stop)
        .await
        .unwrap();
    drop(command_tx);

    timeout(
        Duration::from_secs(4),
        run_worker_request(temp_dir.path().to_path_buf(), request, command_rx, event_tx),
    )
    .await
    .expect("accepted native stop must use the bounded hard-stop fallback")
    .unwrap();
    assert!(collect_worker_events(&mut event_rx)
        .await
        .iter()
        .any(|event| matches!(event, ExternalCliWorkerEvent::Stopped)));
}

#[tokio::test]
async fn worker_registration_wait_and_abort_helpers_cover_unregistered_runs() {
    let _registry_guard = external_cli_env_guard_async().await;
    let completed =
        tokio::spawn(async { Ok(ExternalCliRunResult::stopped(None, "mock".to_string())) });
    while !completed.is_finished() {
        tokio::task::yield_now().await;
    }
    assert!(
        !wait_for_worker_run_stop_attempt(&completed, || async { false }).await,
        "a completed unregistered run must not report a native stop"
    );
    completed.await.unwrap().unwrap();

    let pending = tokio::spawn(std::future::pending::<Result<ExternalCliRunResult, String>>());
    assert!(
        !wait_for_worker_run_stop_attempt(&pending, || async { false }).await,
        "an unregistered run must stop waiting at the bounded deadline"
    );
    assert!(matches!(
        abort_worker_run(&pending),
        ExternalCliWorkerEvent::Stopped
    ));
    assert!(pending.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn stop_worker_sessions_cancels_queued_and_acknowledged_active_entries() {
    let _registry_guard = external_cli_env_guard_async().await;
    let queued_key = format!("queued-stop-all-{}", uuid::Uuid::new_v4());
    let active_key = format!("active-stop-all-{}", uuid::Uuid::new_v4());
    let (cancel_tx, cancel_rx) = watch::channel(false);
    QUEUED_WORKER_SESSIONS.insert(
        queued_key.clone(),
        QueuedExternalCliWorkerControl {
            queue_id: uuid::Uuid::new_v4().to_string(),
            cancel_tx,
        },
    );
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel();
    let (guide_tx, _guide_rx) = tokio::sync::mpsc::channel(1);
    ACTIVE_WORKER_SESSIONS.insert(
        active_key.clone(),
        ExternalCliWorkerControlHandle {
            pid: 1,
            stop_tx,
            guide_tx,
            model_tx: unused_model_tx(),
        },
    );
    let acknowledge = tokio::spawn(async move {
        if let Some(ack_tx) = stop_rx.recv().await {
            let _ = ack_tx.send(());
        }
    });

    let stopped =
        stop_worker_sessions(HashSet::from([queued_key.clone(), active_key.clone()])).await;

    assert!(stopped >= 2);
    assert!(*cancel_rx.borrow());
    acknowledge.await.unwrap();
    QUEUED_WORKER_SESSIONS.remove(&queued_key);
    ACTIVE_WORKER_SESSIONS.remove(&active_key);
}

#[tokio::test]
async fn stop_all_worker_sessions_collects_both_registries() {
    let _registry_guard = external_cli_env_guard_async().await;
    let queued_key = format!("queued-stop-public-{}", uuid::Uuid::new_v4());
    let active_key = format!("active-stop-public-{}", uuid::Uuid::new_v4());
    let (cancel_tx, cancel_rx) = watch::channel(false);
    QUEUED_WORKER_SESSIONS.insert(
        queued_key.clone(),
        QueuedExternalCliWorkerControl {
            queue_id: uuid::Uuid::new_v4().to_string(),
            cancel_tx,
        },
    );
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel();
    let (guide_tx, _guide_rx) = tokio::sync::mpsc::channel(1);
    ACTIVE_WORKER_SESSIONS.insert(
        active_key.clone(),
        ExternalCliWorkerControlHandle {
            pid: 999_999_989,
            stop_tx,
            guide_tx,
            model_tx: unused_model_tx(),
        },
    );
    let acknowledge = tokio::spawn(async move {
        let ack_tx = stop_rx.recv().await.expect("public stop-all request");
        let _ = ack_tx.send(());
    });

    assert_eq!(stop_all_worker_sessions().await, 2);
    assert!(*cancel_rx.borrow());
    acknowledge.await.unwrap();
    QUEUED_WORKER_SESSIONS.remove(&queued_key);
    ACTIVE_WORKER_SESSIONS.remove(&active_key);
}

#[tokio::test]
async fn worker_client_override_and_process_spawn_use_configured_executable() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let _data_dir = EnvGuard::set("BIFROST_DATA_DIR", temp_dir.path());
    #[cfg(unix)]
    let executable = PathBuf::from("/usr/bin/true");
    #[cfg(windows)]
    let executable = PathBuf::from(
        std::env::var_os("SystemRoot").expect("Windows SystemRoot for test executable"),
    )
    .join("System32")
    .join("where.exe");
    let _override_executable = EnvGuard::set(TEST_EXTERNAL_CLI_WORKER_EXECUTABLE_ENV, &executable);

    let client = ExternalCliWorkerClient::current_exe().expect("worker client override");
    assert_eq!(client.executable, executable);
    let mut child = spawn_external_cli_worker_process(
        &client.executable,
        temp_dir.path(),
        &temp_dir.path().join("request.json"),
    )
    .expect("spawn configured external worker executable");
    let status = child.wait().await.expect("wait configured worker");
    assert!(
        status.code().is_some(),
        "configured worker should exit normally"
    );
    #[cfg(unix)]
    assert!(status.success());
}

#[cfg(unix)]
#[tokio::test]
async fn worker_client_spawn_and_event_reader_report_transport_failures() {
    use std::os::unix::fs::PermissionsExt;

    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let _data_dir = EnvGuard::set("BIFROST_DATA_DIR", temp_dir.path());

    let missing_client = ExternalCliWorkerClient {
        executable: temp_dir.path().join("missing-worker"),
    };
    let error = match missing_client
        .spawn(
            temp_dir.path().join("missing-runs"),
            worker_transport_test_request("missing-worker"),
        )
        .await
    {
        Ok(_) => panic!("missing external worker unexpectedly spawned"),
        Err(error) => error,
    };
    assert!(
        error.contains("spawn external runner worker failed"),
        "{error}"
    );

    let executable = temp_dir.path().join("worker-output.sh");
    std::fs::write(
        &executable,
        r#"#!/bin/sh
IFS= read -r _command
case "$BIFROST_TEST_WORKER_OUTPUT_MODE" in
  started)
    sleep 30 &
    nested_pid=$!
    printf '%s\n' "{\"type\":\"started\",\"sessionKey\":\"transport-test\",\"pid\":$$}"
    wait "$nested_pid"
    ;;
  malformed)
    printf '%s\n' 'not-json'
    ;;
  invalid_utf8)
    printf '\377\n'
    ;;
  eof)
    exit 0
    ;;
  eof_with_stderr)
    printf '%s\n' 'worker bootstrap rejected command: missing field request' >&2
    exit 1
    ;;
esac
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();
    let _executable_override = EnvGuard::set(TEST_EXTERNAL_CLI_WORKER_EXECUTABLE_ENV, &executable);
    let client = ExternalCliWorkerClient { executable };

    for mode in [
        "started",
        "malformed",
        "invalid_utf8",
        "eof",
        "eof_with_stderr",
    ] {
        let _mode = EnvGuard::set("BIFROST_TEST_WORKER_OUTPUT_MODE", Path::new(mode));
        let mut run = client
            .spawn(
                temp_dir.path().join(format!("runs-{mode}")),
                worker_transport_test_request(mode),
            )
            .await
            .unwrap();
        let request_path = run.request_path.clone().unwrap();
        match mode {
            "started" => {
                let event = run.next_event().await.unwrap();
                let ExternalCliWorkerEvent::Started { pid, .. } = event else {
                    panic!("expected started event");
                };
                assert!(unix_process_exists(pid), "worker command should be running");
                assert!(!request_path.exists());
                drop(run);
                timeout(Duration::from_secs(3), async {
                    while unix_process_exists(pid) {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
                .await
                .expect("dropping a failed worker transport must reap its nested command");
            }
            "malformed" => {
                let error = run.next_event().await.unwrap_err();
                assert!(
                    error.contains("parse external runner worker event failed"),
                    "{error}"
                );
            }
            "invalid_utf8" => {
                let error = run.next_event().await.unwrap_err();
                assert!(
                    error.contains("read external runner worker event failed"),
                    "{error}"
                );
                assert!(!request_path.exists());
            }
            "eof" => {
                let error = run.next_event().await.unwrap_err();
                assert!(error.contains("exited before final event"), "{error}");
                assert!(!request_path.exists());
            }
            "eof_with_stderr" => {
                let error = run.next_event().await.unwrap_err();
                assert!(error.contains("exited before final event"), "{error}");
                assert!(
                    error.contains("worker bootstrap rejected command: missing field request"),
                    "{error}"
                );
                assert!(!request_path.exists());
            }
            _ => unreachable!(),
        }
    }
}

#[cfg(unix)]
fn unix_process_exists(pid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    matches!(
        kill(Pid::from_raw(pid as i32), None),
        Ok(()) | Err(Errno::EPERM)
    )
}

#[cfg(unix)]
#[tokio::test]
async fn worker_transport_enforces_frame_limits_and_rotates_stderr() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let _data_dir = EnvGuard::set("BIFROST_DATA_DIR", temp_dir.path());

    let mut child = Command::new("sh")
        .arg("-c")
        .arg("sleep 30")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let command_error = write_external_cli_worker_command(
        &mut stdin,
        &ExternalCliWorkerCommand::Guide {
            guide_id: "oversized-guide".to_string(),
            message: "x".repeat(EXTERNAL_CLI_WORKER_MAX_FRAME_BYTES),
        },
    )
    .await
    .unwrap_err();
    assert!(command_error.contains("command exceeds hard limit"));
    let _ = child.kill().await;

    let event_error = send_external_cli_worker_event(&ExternalCliWorkerEvent::Progress {
        event: ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::Status,
            content: "x".repeat(EXTERNAL_CLI_WORKER_MAX_FRAME_BYTES),
            title: None,
            raw: serde_json::Value::Null,
        },
    })
    .unwrap_err();
    assert!(event_error.contains("event exceeds hard limit"));

    let log_dir = external_cli_worker_runtime_root().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    let log_path = log_dir.join("external-cli-worker.log");
    std::fs::File::create(&log_path)
        .unwrap()
        .set_len(EXTERNAL_CLI_WORKER_LOG_MAX_BYTES)
        .unwrap();
    drop(external_cli_worker_stderr_file().unwrap());
    assert!(log_dir.join("external-cli-worker.log.1").is_file());
    assert_eq!(std::fs::metadata(log_path).unwrap().len(), 0);
}

#[tokio::test]
async fn shutdown_all_active_runs_handles_acknowledged_and_closed_workers() {
    let _registry_guard = external_cli_env_guard_async().await;
    let acknowledged_pid = 999_999_991;
    let closed_pid = 999_999_992;
    let (acknowledged_tx, mut acknowledged_rx) = tokio::sync::mpsc::unbounded_channel();
    let (closed_tx, closed_rx) = tokio::sync::mpsc::unbounded_channel();
    drop(closed_rx);
    ACTIVE_WORKERS.insert(acknowledged_pid, acknowledged_tx);
    ACTIVE_WORKERS.insert(closed_pid, closed_tx);

    let acknowledge = tokio::spawn(async move {
        let ack_tx = acknowledged_rx.recv().await.expect("shutdown stop request");
        let _ = ack_tx.send(());
    });
    let acknowledged_stop_tx = ACTIVE_WORKERS
        .get(&acknowledged_pid)
        .map(|entry| entry.value().clone())
        .unwrap();
    let closed_stop_tx = ACTIVE_WORKERS
        .get(&closed_pid)
        .map(|entry| entry.value().clone())
        .unwrap();

    drop(acknowledged_stop_tx);
    drop(closed_stop_tx);
    shutdown_all_active_runs().await;

    acknowledge.await.unwrap();
    assert!(!ACTIVE_WORKERS.contains_key(&acknowledged_pid));
    assert!(!ACTIVE_WORKERS.contains_key(&closed_pid));
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_all_active_runs_terminates_owned_unacknowledged_worker() {
    let _registry_guard = external_cli_env_guard_async().await;
    use std::os::unix::process::CommandExt;
    use std::process::Command as StdProcessCommand;

    let mut child = StdProcessCommand::new("sh")
        .arg("-c")
        .arg("sleep 30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn shutdown fallback worker");
    let pid = child.id();
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel();
    ACTIVE_WORKERS.insert(pid, stop_tx.clone());
    let consume_without_ack = tokio::spawn(async move {
        let _ack_tx = stop_rx.recv().await.expect("shutdown stop request");
        tokio::time::sleep(Duration::from_secs(10)).await;
    });

    drop(stop_tx);
    shutdown_all_active_runs().await;

    let status = child.wait().expect("reap shutdown fallback worker");
    assert!(!status.success());
    assert!(!ACTIVE_WORKERS.contains_key(&pid));
    consume_without_ack.abort();
}

#[test]
fn active_worker_registration_drop_removes_matching_session_owner() {
    let _registry_guard = external_cli_env_guard();
    let pid = 999_999_987;
    let session_key = format!("matching-drop-owner-{}", uuid::Uuid::new_v4());
    let (stop_tx, _stop_rx) = tokio::sync::mpsc::unbounded_channel();
    let (guide_tx, _guide_rx) = tokio::sync::mpsc::channel(1);
    ACTIVE_WORKERS.insert(pid, stop_tx.clone());
    ACTIVE_WORKER_SESSIONS.insert(
        session_key.clone(),
        ExternalCliWorkerControlHandle {
            pid,
            stop_tx: stop_tx.clone(),
            guide_tx,
            model_tx: unused_model_tx(),
        },
    );

    drop(ActiveWorkerRegistration {
        pid,
        session_key: Some(session_key.clone()),
        stop_tx,
    });

    assert!(!ACTIVE_WORKERS.contains_key(&pid));
    assert!(!ACTIVE_WORKER_SESSIONS.contains_key(&session_key));
}

#[test]
fn worker_session_lock_index_is_stable_and_bounded() {
    let left = worker_session_lock_index("provider:session");
    let right = worker_session_lock_index("provider:session");
    assert_eq!(left, right);
    assert!(left < WORKER_SESSION_LOCK_STRIPES);
}

#[test]
fn stale_worker_registration_cannot_remove_reused_pid_owner() {
    let _registry_guard = external_cli_env_guard();
    let pid = 999_999_990;
    let session_key = format!("pid-reuse-owner-{}", uuid::Uuid::new_v4());
    let (old_stop_tx, _old_stop_rx) = tokio::sync::mpsc::unbounded_channel();
    let (new_stop_tx, _new_stop_rx) = tokio::sync::mpsc::unbounded_channel();
    let (guide_tx, _guide_rx) = tokio::sync::mpsc::channel(1);
    ACTIVE_WORKERS.insert(pid, new_stop_tx.clone());
    ACTIVE_WORKER_SESSIONS.insert(
        session_key.clone(),
        ExternalCliWorkerControlHandle {
            pid,
            stop_tx: new_stop_tx.clone(),
            guide_tx,
            model_tx: unused_model_tx(),
        },
    );

    drop(ActiveWorkerRegistration {
        pid,
        session_key: Some(session_key.clone()),
        stop_tx: old_stop_tx,
    });

    assert!(active_worker_is_owned(pid, &new_stop_tx));
    assert!(ACTIVE_WORKER_SESSIONS
        .get(&session_key)
        .is_some_and(|handle| handle.stop_tx.same_channel(&new_stop_tx)));
    assert!(remove_active_worker_if_owned(pid, &new_stop_tx));
    ACTIVE_WORKER_SESSIONS.remove(&session_key);
}

#[test]
fn external_cli_adapter_parser_maps_progress_events() {
    let stdout = r#"{"type":"run_started","content":"start"}
{"type":"assistant_delta","delta":"hello"}
not json
{"type":"tool_started","tool_name":"exec_command","content":"running"}
{"type":"assistant_final","content":"done"}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(events.len(), 4);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::RunStarted
    );
    assert_eq!(
        events[1].event_type,
        ExternalCliProgressEventType::AssistantDelta
    );
    assert_eq!(events[1].content, "hello");
    assert_eq!(events[2].title.as_deref(), Some("exec_command"));
    assert_eq!(
        events[3].event_type,
        ExternalCliProgressEventType::AssistantFinal
    );
}

#[test]
fn codex_cli_parser_maps_real_jsonl_events() {
    let stdout = r#"{"type":"thread.started","thread_id":"thread-1"}
{"type":"item.completed","item":{"id":"item_0","type":"error","message":"deprecated config warning"}}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"BIFROST_REAL_CODEX_OK"}}
{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(events.len(), 5);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::RunStarted
    );
    assert_eq!(events[1].event_type, ExternalCliProgressEventType::Status);
    assert_eq!(events[1].content, "deprecated config warning");
    assert_eq!(events[2].event_type, ExternalCliProgressEventType::Status);
    assert_eq!(
        events[3].event_type,
        ExternalCliProgressEventType::AssistantFinal
    );
    assert_eq!(events[3].content, "BIFROST_REAL_CODEX_OK");
    assert_eq!(
        events[4].event_type,
        ExternalCliProgressEventType::RunFinished
    );
}

#[test]
fn codex_cli_parser_maps_real_todo_list_events_to_plan_updates() {
    let stdout = r#"{"type":"item.started","item":{"id":"item_0","type":"todo_list","items":[{"text":"inspect output","completed":false},{"text":"map parser","completed":false},{"text":"verify UI","completed":false}]}}
{"type":"item.updated","item":{"id":"item_0","type":"todo_list","items":[{"text":"inspect output","completed":true},{"text":"map parser","completed":false},{"text":"verify UI","completed":false}]}}
{"type":"item.completed","item":{"id":"item_0","type":"todo_list","items":[{"text":"inspect output","completed":true},{"text":"map parser","completed":false},{"text":"verify UI","completed":false}]}}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(events.len(), 3);
    assert!(events
        .iter()
        .all(|event| event.event_type == ExternalCliProgressEventType::PlanUpdated));
    assert!(events.iter().all(|event| event.title.is_none()));
    let initial_steps = external_progress_plan_steps(&events[0]);
    assert_eq!(initial_steps.len(), 3);
    assert_eq!(initial_steps[0].step, "inspect output");
    assert_eq!(initial_steps[0].status, PlanStepStatus::Pending);
    let updated_steps = external_progress_plan_steps(&events[1]);
    assert_eq!(updated_steps[0].status, PlanStepStatus::Completed);
    assert_eq!(
        updated_steps[1].status,
        PlanStepStatus::Pending,
        "Codex todo_list currently exposes completed=true/false, not in_progress"
    );
}

#[test]
fn codex_and_traex_collab_events_are_plain_tool_input_and_output() {
    let stdout = r#"{"type":"item.started","item":{"id":"collab-1","type":"collab_agent_tool_call","tool":"spawnAgent","status":"in_progress","prompt":"Review the authentication flow","arguments":{"prompt":"Review the authentication flow","receiver_thread_ids":["agent-7"],"agents_states":{"agent-7":{"status":"starting"}}},"sender_thread_id":"root","receiver_thread_ids":[],"agents_states":{}}}
{"type":"item.updated","item":{"id":"collab-1","type":"collab_agent_tool_call","tool":"spawnAgent","status":"in_progress","prompt":"Review the authentication flow","sender_thread_id":"root","receiver_thread_ids":["agent-7"],"agents_states":{"agent-7":{"status":"running","message":"Inspecting handlers"}}}}
{"type":"item.completed","item":{"id":"collab-1","type":"collab_agent_tool_call","tool":"spawnAgent","status":"completed","prompt":"Review the authentication flow","result":"Review complete","sender_thread_id":"root","receiver_thread_ids":["agent-7"],"agents_states":{"agent-7":{"status":"completed","message":"internal detail must stay hidden"}}}}"#;

    let events = parse_progress_events(stdout);
    assert_eq!(events.len(), 2, "item.updated is internal lifecycle noise");
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::ToolStarted
    );
    assert_eq!(events[0].title.as_deref(), Some("spawnAgent"));
    assert_eq!(
        events[0].raw["arguments"]["prompt"],
        "Review the authentication flow"
    );
    assert!(events[0].raw["arguments"]
        .get("receiver_thread_ids")
        .is_none());
    assert!(events[0].raw["arguments"].get("agents_states").is_none());
    assert!(!events[0].raw.to_string().contains("agent-7"));
    assert_eq!(
        events[1].event_type,
        ExternalCliProgressEventType::ToolFinished
    );
    assert_eq!(events[1].content, "Review complete");
    assert_eq!(events[1].raw["success"], true);
    assert!(!events[1].content.contains("internal detail"));
    assert!(!events[1].raw.to_string().contains("agent-7"));
    assert!(!events[1].raw.to_string().contains("internal detail"));

    let mapped = external_progress_to_agent_turn_event(
        "session",
        TRAEX_ADAPTER,
        ExternalCliProgressStatusContext::new(None, None, None, None, None, None),
        &events[0],
    )
    .expect("plain tool event");
    assert!(matches!(
        mapped,
        bifrost_agent::AgentTurnProgressEvent::ToolStarted { .. }
    ));
}

#[test]
fn codex_subagent_activity_is_not_user_visible() {
    let events = parse_progress_events(
        r#"{"type":"item.started","item":{"id":"activity-1","type":"sub_agent_activity","agent_thread_id":"agent-9","agent_path":"reviewer","kind":"started"}}
{"type":"item.completed","item":{"id":"activity-2","type":"sub_agent_activity","agent_thread_id":"agent-9","agent_path":"reviewer","kind":"interrupted"}}"#,
    );
    assert!(events.is_empty());
}

#[test]
fn codex_collab_completion_does_not_expand_internal_agent_states() {
    let events = parse_progress_events(
        r#"{"type":"item.completed","item":{"id":"wait-9","type":"collab_agent_tool_call","tool":"wait","status":"completed","result":"Wait completed","sender_thread_id":"root","receiver_thread_ids":["agent-1","agent-2"],"agents_states":{"agent-1":{"status":"completed","message":"Review complete"},"agent-2":{"status":"running","message":"Still testing"}}}}"#,
    );

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::ToolFinished
    );
    assert_eq!(events[0].content, "Wait completed");
    assert!(!events[0].content.contains("Still testing"));
}

#[test]
fn codex_failed_and_claude_interrupted_collaboration_tools_keep_failure_state() {
    let failed = parse_progress_events(
        r#"{"type":"item.completed","item":{"id":"collab-failed","type":"collab_agent_tool_call","tool":"wait","status":"failed","prompt":"Review auth","error":"Permission denied","receiver_thread_ids":["agent-failed"],"agents_states":{"agent-failed":{"status":"errored","message":"internal"}}}}"#,
    );
    assert_eq!(
        failed[0].event_type,
        ExternalCliProgressEventType::ToolFinished
    );
    assert_eq!(failed[0].content, "Permission denied");
    assert_eq!(failed[0].raw["success"], false);

    let interrupted = parse_progress_events(
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"task-interrupted","name":"Agent","input":{"description":"Inspect auth","prompt":"Review auth","subagent_type":"reviewer"}}]}}
{"type":"user","message":{"content":[{"tool_use_id":"task-interrupted","type":"tool_result","content":"Stopped by parent","is_error":true}]},"tool_use_result":{"agentId":"claude-agent-2","totalDurationMs":2500,"interrupted":true}}"#,
    );
    assert_eq!(
        interrupted[0].event_type,
        ExternalCliProgressEventType::ToolStarted
    );
    assert_eq!(
        interrupted[1].event_type,
        ExternalCliProgressEventType::ToolFinished
    );
    assert_eq!(interrupted[1].content, "Stopped by parent");
    assert_eq!(interrupted[1].raw["success"], false);
}

#[test]
fn generic_plan_updated_parser_accepts_status_fields() {
    let events = parse_progress_events(
        r#"{"type":"plan_updated","title":"Runner plan","items":[{"text":"inspect","status":"completed"},{"text":"map","status":"in_progress"},{"text":"verify","status":"pending"}]}"#,
    );

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::PlanUpdated
    );
    assert_eq!(events[0].title.as_deref(), Some("Runner plan"));
    let steps = external_progress_plan_steps(&events[0]);
    assert_eq!(steps[0].status, PlanStepStatus::Completed);
    assert_eq!(steps[1].status, PlanStepStatus::InProgress);
    assert_eq!(steps[2].status, PlanStepStatus::Pending);
}

#[test]
fn codex_cli_parser_maps_real_command_execution_events() {
    let stdout = r#"{"type":"thread.started","thread_id":"019ea049-6138-7303-ab6e-dacccbd437a7"}
{"type":"turn.started"}
{"type":"item.started","item":{"id":"item_0","type":"command_execution","command":"/bin/zsh -lc pwd","aggregated_output":"","exit_code":null,"status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_0","type":"command_execution","command":"/bin/zsh -lc pwd","aggregated_output":"/Users/eden/work/github/bifrost-traex-runner\n","exit_code":0,"status":"completed"}}
{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"BIFROST_CODEX_REALTIME_DIRECT_OK"}}
{"type":"turn.completed","usage":{"input_tokens":59589,"cached_input_tokens":6912,"output_tokens":221,"reasoning_output_tokens":156}}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(events.len(), 6);
    assert_eq!(
        events[2].event_type,
        ExternalCliProgressEventType::ToolStarted
    );
    assert_eq!(events[2].title.as_deref(), Some("exec_command"));
    assert_eq!(events[2].content, "/bin/zsh -lc pwd");
    assert_eq!(
        events[2]
            .raw
            .get("arguments")
            .and_then(|value| value.get("command"))
            .and_then(serde_json::Value::as_str),
        Some("/bin/zsh -lc pwd")
    );
    assert_eq!(
        events[3].event_type,
        ExternalCliProgressEventType::ToolFinished
    );
    assert_eq!(events[3].title.as_deref(), Some("exec_command"));
    assert_eq!(
        events[3].content,
        "/Users/eden/work/github/bifrost-traex-runner\n"
    );
    assert_eq!(
        events[3]
            .raw
            .get("success")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        events[4].event_type,
        ExternalCliProgressEventType::AssistantFinal
    );
    assert_eq!(events[4].content, "BIFROST_CODEX_REALTIME_DIRECT_OK");
    assert_eq!(
        events[5].event_type,
        ExternalCliProgressEventType::RunFinished
    );
}

#[test]
fn file_change_detail_counts_added_deleted_and_modified_lines() {
    let detail = file_change_detail_from_value(&serde_json::json!({
        "type": "fileChange",
        "changes": [
            {
                "path": "src/updated.rs",
                "kind": {"type": "update", "move_path": null},
                "diff": "--- a/src/updated.rs\n+++ b/src/updated.rs\n@@ -1,2 +1,3 @@\n-old\n+new\n+extra\n context\n"
            },
            {
                "path": "src/new.rs",
                "kind": {"type": "add"},
                "diff": "--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1,2 @@\n+one\n+two\n"
            },
            {
                "path": "src/old.rs",
                "kind": {"type": "delete"},
                "diff": "--- a/src/old.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\n-two\n"
            }
        ]
    }))
    .expect("file change detail");

    assert!(detail.contains("file: src/updated.rs (修改 1 行 · 新增 1 行)"));
    assert!(detail.contains("file: src/new.rs (新增 2 行)"));
    assert!(detail.contains("file: src/old.rs (删除 2 行)"));
    assert!(!detail.contains("修改 2 行 · 新增 1 行 · 删除 1 行"));
}

#[test]
fn file_change_detail_keeps_action_when_diff_has_no_changed_lines() {
    let detail = file_change_detail_from_value(&serde_json::json!({
        "path": "src/renamed.rs",
        "status": "completed",
        "kind": {"type": "move", "move_path": "src/original.rs"}
    }))
    .expect("file change detail");

    assert_eq!(detail, "file: src/renamed.rs (移动)");
}

#[test]
fn file_change_detail_counts_plain_added_and_deleted_content() {
    let detail = file_change_detail_from_value(&serde_json::json!({
        "type": "fileChange",
        "changes": [
            {
                "path": "/workspace/src/new.rs",
                "kind": {"type": "add"},
                "diff": "first\n+literal content\n-third\n"
            },
            {
                "path": "/workspace/src/old.rs",
                "kind": {"type": "delete"},
                "diff": "first\n\nthird\n"
            }
        ]
    }))
    .expect("file change detail");

    assert!(detail.contains("file: /workspace/src/new.rs (新增 3 行)"));
    assert!(detail.contains("file: /workspace/src/old.rs (删除 3 行)"));
}

#[test]
fn file_change_detail_uses_workspace_relative_paths_and_indents_every_diff_line() {
    let detail = file_change_detail_from_value_with_work_dir(
        &serde_json::json!({
            "type": "fileChange",
            "changes": [{
                "path": "/workspace/project/target/demo.txt",
                "kind": {"type": "add"},
                "diff": "first\nsecond\nthird\n"
            }]
        }),
        Some(Path::new("/workspace/project")),
    )
    .expect("file change detail");

    assert_eq!(
        detail,
        "changes:\n- file: target/demo.txt (新增 3 行)\n  first\n  second\n  third"
    );
}

#[test]
fn file_change_detail_preserves_paths_outside_workspace() {
    let detail = file_change_detail_from_value_with_work_dir(
        &serde_json::json!({
            "type": "fileChange",
            "changes": [{
                "path": "/shared/demo.txt",
                "kind": {"type": "add"},
                "diff": "one\n"
            }]
        }),
        Some(Path::new("/workspace/project")),
    )
    .expect("file change detail");

    assert!(detail.contains("file: /shared/demo.txt (新增 1 行)"));
}

#[test]
fn file_change_detail_preserves_unknown_actions_and_path_only_changes() {
    let detail = file_change_detail_from_value(&serde_json::json!({
        "changes": [
            {"path": "scripts/tool.sh", "action": "chmod"},
            {"path": "assets/empty.txt"}
        ]
    }))
    .expect("file change detail");

    assert!(detail.contains("file: scripts/tool.sh (chmod)"));
    assert!(detail.contains("file: assets/empty.txt"));
}

#[test]
fn file_change_line_stats_do_not_pair_changes_across_hunks() {
    let diff = "@@ -1 +1 @@\n-old\n context\n@@ -8 +8,2 @@\n context\n+new\n";

    assert_eq!(unified_diff_line_stats(diff), (1, 1, 0));
}

#[test]
fn external_progress_result_prefers_file_detail_and_keeps_structured_fallbacks() {
    let nested_file_change = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolFinished,
        content: "stale absolute-path detail".to_string(),
        title: Some("fileChange".to_string()),
        raw: serde_json::json!({
            "params": {
                "item": {
                    "type": "fileChange",
                    "path": "/workspace/project/src/main.rs",
                    "kind": {"type": "update"}
                }
            }
        }),
    };
    assert_eq!(
        external_progress_result_text(&nested_file_change, Some(Path::new("/workspace/project"))),
        "file: src/main.rs (修改)"
    );

    let detail_free_file_change = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolFinished,
        content: String::new(),
        title: Some("fileChange".to_string()),
        raw: serde_json::json!({"item": {"type": "fileChange"}}),
    };
    assert!(
        external_progress_result_text(&detail_free_file_change, None)
            .contains(r#""type": "fileChange""#)
    );

    let empty_regular_tool = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolFinished,
        content: String::new(),
        title: Some("exec_command".to_string()),
        raw: serde_json::json!({}),
    };
    assert!(external_progress_result_text(&empty_regular_tool, None).is_empty());
}

#[test]
fn file_change_detail_covers_top_level_diff_and_header_only_unified_diff() {
    let detail = file_change_detail_from_value(&serde_json::json!({
        "diff": "--- a/src/main.rs\n+++ b/src/main.rs\n-old\n+new\n"
    }))
    .expect("top-level diff detail");
    assert_eq!(
        detail,
        "diff:\n  --- a/src/main.rs\n  +++ b/src/main.rs\n  -old\n  +new"
    );
    assert!(looks_like_unified_diff(
        "--- a/src/main.rs\n+++ b/src/main.rs\n-old\n+new\n"
    ));

    assert_eq!(
        format_file_change_path("src/main.rs", Some("修改"), Some("context only"), None),
        "file: src/main.rs (修改)"
    );
}

#[test]
fn traex_cli_parser_maps_real_jsonl_events() {
    let stdout = r#"{"type":"thread.started","thread_id":"019e9f78-traex"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_0","type":"error","message":"model rerouted"}}
{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"BIFROST_TRAEX_OK"}}
{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(events.len(), 5);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::RunStarted
    );
    assert_eq!(events[0].content, "019e9f78-traex");
    assert_eq!(events[1].event_type, ExternalCliProgressEventType::Status);
    assert_eq!(events[2].event_type, ExternalCliProgressEventType::Status);
    assert_eq!(events[2].content, "model rerouted");
    assert_eq!(
        events[3].event_type,
        ExternalCliProgressEventType::AssistantFinal
    );
    assert_eq!(events[3].content, "BIFROST_TRAEX_OK");
}

#[test]
fn claude_code_parser_maps_stream_json_events() {
    let stdout = r#"{"type":"system","subtype":"init","session_id":"claude-session-1"}
{"type":"assistant","message":{"content":[{"type":"text","text":"BIFROST_CLAUDE_CODE_OK"}],"usage":{"input_tokens":10,"output_tokens":4}}}
{"type":"result","subtype":"success","is_error":false,"result":"BIFROST_CLAUDE_CODE_OK","session_id":"claude-session-1","usage":{"input_tokens":10,"output_tokens":4}}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::RunStarted
    );
    assert_eq!(events[0].content, "claude-session-1");
    assert_eq!(
        events[1].event_type,
        ExternalCliProgressEventType::AssistantFinal
    );
    assert_eq!(events[1].content, "BIFROST_CLAUDE_CODE_OK");
    assert_eq!(
        events[2].event_type,
        ExternalCliProgressEventType::RunFinished
    );

    let mut metadata = std::collections::BTreeMap::new();
    append_external_cli_metadata(CLAUDE_CODE_ADAPTER, &events, &mut metadata);

    assert_eq!(
        metadata.get("threadId").map(String::as_str),
        Some("claude-session-1")
    );
    assert_eq!(
        metadata.get("usageInputTokens").map(String::as_str),
        Some("10")
    );
    assert_eq!(
        metadata.get("usageOutputTokens").map(String::as_str),
        Some("4")
    );
    assert_eq!(
        metadata.get("usageTotalTokens").map(String::as_str),
        Some("14")
    );
}

#[test]
fn claude_code_parser_maps_tool_use_and_tool_result() {
    let stdout = r#"{"type":"system","subtype":"init","session_id":"claude-session-tool"}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tooluse_1","name":"Bash","input":{"command":"pwd"}}]}}
{"type":"user","message":{"content":[{"tool_use_id":"tooluse_1","type":"tool_result","content":"/Users/bytedance/project/bifrost","is_error":false}]},"tool_use_result":{"stdout":"/Users/bytedance/project/bifrost","stderr":"","interrupted":false}}
{"type":"assistant","message":{"content":[{"type":"text","text":"BIFROST_CLAUDE_TOOL_OK"}]}}
{"type":"result","subtype":"success","is_error":false,"result":"BIFROST_CLAUDE_TOOL_OK","session_id":"claude-session-tool","usage":{"input_tokens":20,"output_tokens":6}}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(
        events
            .iter()
            .map(|event| &event.event_type)
            .collect::<Vec<_>>(),
        vec![
            &ExternalCliProgressEventType::RunStarted,
            &ExternalCliProgressEventType::ToolStarted,
            &ExternalCliProgressEventType::ToolFinished,
            &ExternalCliProgressEventType::AssistantFinal,
            &ExternalCliProgressEventType::RunFinished,
        ]
    );
    assert_eq!(events[1].title.as_deref(), Some("Bash"));
    assert_eq!(events[1].content, "pwd");
    assert_eq!(events[2].title.as_deref(), Some("Bash"));
    assert_eq!(events[2].content, "/Users/bytedance/project/bifrost");
    assert_eq!(external_progress_arguments_text(&events[2]), "pwd");
    assert_eq!(
        events[2]
            .raw
            .get("success")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );

    let context = ExternalCliProgressStatusContext::new(
        Some(DEFAULT_CLAUDE_CODE_RUNNER_ID),
        None,
        None,
        None,
        None,
        None,
    );
    let turn_started =
        external_progress_to_agent_turn_event("session", CLAUDE_CODE_ADAPTER, context, &events[1])
            .expect("tool started event");
    assert!(matches!(
        turn_started,
        bifrost_agent::AgentTurnProgressEvent::ToolStarted { .. }
    ));
    let turn_finished =
        external_progress_to_agent_turn_event("session", CLAUDE_CODE_ADAPTER, context, &events[2])
            .expect("tool finished event");
    match turn_finished {
        bifrost_agent::AgentTurnProgressEvent::ToolFinished { log, .. } => {
            assert_eq!(log.tool_name, "Bash");
            assert_eq!(log.arguments, "pwd");
            assert_eq!(log.result, "/Users/bytedance/project/bifrost");
            assert!(log.success);
        }
        other => panic!("expected tool finished event, got {other:?}"),
    }
}

#[test]
fn claude_code_task_tool_uses_plain_tool_events() {
    let stdout = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"task_1","name":"Task","input":{"description":"Inspect auth","prompt":"Review the authentication flow and report risks","subagent_type":"security-reviewer"}}]}}
{"type":"user","message":{"content":[{"tool_use_id":"task_1","type":"tool_result","content":"Found no blocker","is_error":false}]},"tool_use_result":{"agentId":"claude-agent-1","totalDurationMs":4200,"interrupted":false}}"#;

    let events = parse_progress_events(stdout);
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::ToolStarted
    );
    assert_eq!(events[0].title.as_deref(), Some("Task"));
    assert_eq!(
        events[0].raw["arguments"]["prompt"],
        "Review the authentication flow and report risks"
    );
    assert_eq!(
        events[1].event_type,
        ExternalCliProgressEventType::ToolFinished
    );
    assert_eq!(events[1].title.as_deref(), Some("Task"));
    assert_eq!(events[1].content, "Found no blocker");
    assert_eq!(events[1].raw["success"], true);
}

#[test]
fn codex_adapter_builds_exec_command_with_prompt_stdin() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: Some("be concise".to_string()),
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            profile: Some("bifrost".to_string()),
            model: Some("gpt-test".to_string()),
            sandbox: Some("workspace-write".to_string()),
            approval_policy: Some("never".to_string()),
            search: Some(true),
            ephemeral: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert_eq!(spec.executable, "codex");
    assert_eq!(spec.args[0], "exec");
    assert!(spec.args.contains(&"--json".to_string()));
    assert!(has_arg_pair(&spec.args, "--cd", "/tmp/work"));
    assert!(has_arg_pair(&spec.args, "--profile", "bifrost"));
    assert!(has_arg_pair(&spec.args, "--model", "gpt-test"));
    assert!(has_arg_pair(&spec.args, "--sandbox", "workspace-write"));
    assert!(!spec.args.contains(&"--ask-for-approval".to_string()));
    assert!(has_arg_pair(&spec.args, "--enable", "web_search"));
    assert!(spec.args.contains(&"--ephemeral".to_string()));
    assert!(has_arg_pair(
        &spec.args,
        "--output-last-message",
        "/tmp/last.md"
    ));
    assert!(!spec.args.windows(2).any(|pair| {
        pair[0] == "--config" && pair[1].trim_start().starts_with("service_tier=")
    }));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn codex_adapter_defaults_to_danger_full_access_for_headless_runs() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert!(!spec.args.contains(&"--sandbox".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn codex_adapter_respects_explicit_sandbox_without_danger_full_access() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            sandbox: Some("workspace-write".to_string()),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--sandbox", "workspace-write"));
    assert!(!spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn traex_adapter_builds_exec_command_with_prompt_stdin() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("traex".to_string()),
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: TRAEX_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: Some("be concise".to_string()),
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("traex".to_string()),
            profile: Some("bifrost".to_string()),
            model: Some("gpt-test".to_string()),
            sandbox: Some("workspace-write".to_string()),
            permission_mode: Some("auto".to_string()),
            skip_git_repo_check: Some(true),
            ignore_user_config: Some(true),
            ignore_rules: Some(true),
            add_dirs: vec!["/tmp/extra".to_string()],
            config_overrides: vec!["shell_environment_policy.inherit=all".to_string()],
            enable_features: vec!["web_search".to_string()],
            ephemeral: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert_eq!(spec.executable, "traex");
    assert_eq!(spec.timeout_secs, None);
    assert!(has_arg_pair(&spec.args, "--cd", "/tmp/work"));
    assert!(spec.args.contains(&"exec".to_string()));
    assert!(spec.args.contains(&"--json".to_string()));
    assert!(has_arg_pair(
        &spec.args,
        "--output-last-message",
        "/tmp/last.md"
    ));
    assert!(has_arg_pair(&spec.args, "--profile", "bifrost"));
    assert!(has_arg_pair(&spec.args, "--model", "gpt-test"));
    assert!(has_arg_pair(&spec.args, "--sandbox", "workspace-write"));
    assert!(has_arg_pair(&spec.args, "--permission-mode", "auto"));
    assert!(has_arg_pair(&spec.args, "--add-dir", "/tmp/extra"));
    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "shell_environment_policy.inherit=all"
    ));
    assert!(has_arg_pair(&spec.args, "--enable", "web_search"));
    assert!(spec.args.contains(&"--skip-git-repo-check".to_string()));
    assert!(spec.args.contains(&"--ignore-user-config".to_string()));
    assert!(spec.args.contains(&"--ignore-rules".to_string()));
    assert!(spec.args.contains(&"--ephemeral".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn traex_adapter_defaults_to_headless_full_access_for_exec() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("traex".to_string()),
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: TRAEX_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("traex".to_string()),
            skip_git_repo_check: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(!spec.args.contains(&"--permission-mode".to_string()));
    assert!(!spec.args.contains(&"bypass_permissions".to_string()));
    assert!(!spec.args.contains(&"default".to_string()));
    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn traex_adapter_maps_default_permission_mode_to_headless_full_access() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("traex".to_string()),
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: TRAEX_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("traex".to_string()),
            permission_mode: Some("default".to_string()),
            sandbox: Some("workspace-write".to_string()),
            skip_git_repo_check: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(!spec.args.contains(&"--permission-mode".to_string()));
    assert!(!spec.args.contains(&"bypass_permissions".to_string()));
    assert!(!spec.args.contains(&"default".to_string()));
    assert!(!spec.args.contains(&"--sandbox".to_string()));
    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn traex_adapter_respects_explicit_non_bypass_permission_mode() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("traex".to_string()),
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: TRAEX_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("traex".to_string()),
            permission_mode: Some("plan".to_string()),
            sandbox: Some("workspace-write".to_string()),
            skip_git_repo_check: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--permission-mode", "plan"));
    assert!(has_arg_pair(&spec.args, "--sandbox", "workspace-write"));
    assert!(!spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn traex_adapter_builds_resume_command_from_thread_id() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello again".to_string(),
        operation: default_operation(),
        params: serde_json::json!({ "threadId": "thread-existing" }),
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("traex".to_string()),
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: TRAEX_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("traex".to_string()),
            profile: Some("not-supported-by-resume".to_string()),
            model: Some("gpt-test".to_string()),
            sandbox: Some("workspace-write".to_string()),
            permission_mode: Some("bypass_permissions".to_string()),
            danger_full_access: Some(true),
            add_dirs: vec!["/tmp/extra".to_string()],
            ephemeral: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert_eq!(spec.executable, "traex");
    assert!(has_arg_pair(&spec.args, "--cd", "/tmp/work"));
    assert!(spec.args.contains(&"exec".to_string()));
    assert!(spec.args.contains(&"resume".to_string()));
    assert!(spec.args.contains(&"--json".to_string()));
    assert!(has_arg_pair(&spec.args, "--model", "gpt-test"));
    assert!(!spec.args.contains(&"--permission-mode".to_string()));
    assert!(!spec.args.contains(&"bypass_permissions".to_string()));
    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert!(spec.args.contains(&"--ephemeral".to_string()));
    assert!(spec.args.contains(&"thread-existing".to_string()));
    assert!(!spec.args.contains(&"--profile".to_string()));
    assert!(!spec.args.contains(&"--sandbox".to_string()));
    assert!(!spec.args.contains(&"--add-dir".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn codex_adapter_builds_current_cli_config_flags() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            profile_v2: Some("team".to_string()),
            model: Some("gpt-test".to_string()),
            sandbox: Some("workspace-write".to_string()),
            approval_policy: Some("never".to_string()),
            reasoning_effort: Some("high".to_string()),
            reasoning_summary: Some("auto".to_string()),
            dangerously_bypass_hook_trust: Some(true),
            strict_config: Some(true),
            skip_git_repo_check: Some(true),
            ignore_user_config: Some(true),
            ignore_rules: Some(true),
            oss: Some(true),
            local_provider: Some("ollama".to_string()),
            output_schema: Some("/tmp/schema.json".to_string()),
            color: Some("never".to_string()),
            add_dirs: vec!["/tmp/extra".to_string()],
            config_overrides: vec!["shell_environment_policy.inherit=all".to_string()],
            enable_features: vec!["web_search".to_string()],
            disable_features: vec!["legacy_mode".to_string()],
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--profile-v2", "team"));
    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "shell_environment_policy.inherit=all"
    ));
    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "approval_policy=\"never\""
    ));
    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "model_reasoning_effort=\"high\""
    ));
    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "model_reasoning_summary=\"auto\""
    ));
    assert!(has_arg_pair(&spec.args, "--enable", "web_search"));
    assert!(has_arg_pair(&spec.args, "--disable", "legacy_mode"));
    assert!(has_arg_pair(&spec.args, "--add-dir", "/tmp/extra"));
    assert!(spec
        .args
        .contains(&"--dangerously-bypass-hook-trust".to_string()));
    assert!(spec.args.contains(&"--strict-config".to_string()));
    assert!(spec.args.contains(&"--oss".to_string()));
    assert!(has_arg_pair(&spec.args, "--local-provider", "ollama"));
    assert!(has_arg_pair(
        &spec.args,
        "--output-schema",
        "/tmp/schema.json"
    ));
    assert!(has_arg_pair(&spec.args, "--color", "never"));
    assert!(spec.args.contains(&"--skip-git-repo-check".to_string()));
    assert!(spec.args.contains(&"--ignore-user-config".to_string()));
    assert!(spec.args.contains(&"--ignore-rules".to_string()));
    assert!(!spec.args.contains(&"--search".to_string()));
}

#[test]
fn codex_adapter_respects_configured_service_tier_override() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            config_overrides: vec!["service_tier=\"flex\"".to_string()],
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "service_tier=\"flex\""
    ));
    assert!(!has_arg_pair(
        &spec.args,
        "--config",
        "service_tier=\"fast\""
    ));
}

#[test]
fn codex_session_fast_override_replaces_runner_service_tier() {
    let mut request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("Codex".to_string()),
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            config_overrides: vec![
                "service_tier=\"flex\"".to_string(),
                "model_reasoning_effort=\"high\"".to_string(),
            ],
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let state = crate::im_gateway::session_state::ImAgentSessionState {
        service_tier_override: Some(CODEX_STANDARD_SERVICE_TIER.to_string()),
        service_tier_override_source: Some("session slash command".to_string()),
        ..Default::default()
    };

    apply_external_cli_session_overrides_to_run_request(&mut request, Some(&state));
    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "service_tier=\"default\""
    ));
    assert!(!has_arg_pair(
        &spec.args,
        "--config",
        "service_tier=\"flex\""
    ));
    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "model_reasoning_effort=\"high\""
    ));
}

#[test]
fn service_tier_resolution_uses_last_runner_override_without_bifrost_default() {
    let configured = ExternalCliAdapterConfig {
        config_overrides: vec![
            "service_tier=\"flex\"".to_string(),
            "service_tier=\"default\"".to_string(),
        ],
        ..Default::default()
    };

    assert_eq!(
        resolve_external_cli_service_tier(DEFAULT_ADAPTER, &configured),
        (
            Some(CODEX_STANDARD_SERVICE_TIER.to_string()),
            Some("runner config".to_string())
        )
    );
    assert_eq!(
        resolve_external_cli_service_tier(DEFAULT_ADAPTER, &ExternalCliAdapterConfig::default()),
        (None, None)
    );
    assert_eq!(
        resolve_external_cli_service_tier(TRAEX_ADAPTER, &ExternalCliAdapterConfig::default()),
        (None, None)
    );
}

#[test]
fn codex_fast_status_formats_fast_standard_custom_and_default_modes() {
    let fast = format_external_cli_fast_status(
        Some(CODEX_FAST_SERVICE_TIER),
        Some("session slash command"),
        "Codex",
    );
    assert!(fast.contains("使用快速模式"));
    assert!(fast.contains("service tier: `fast`"));
    assert!(fast.contains("来源: session slash command"));

    let standard = format_external_cli_fast_status(
        Some(CODEX_STANDARD_SERVICE_TIER),
        Some("runner config"),
        "Codex",
    );
    assert!(standard.contains("使用标准模式"));
    assert!(standard.contains("service tier: `default`"));
    assert!(standard.contains("来源: runner config"));

    let custom = format_external_cli_fast_status(Some("flex"), None, "Codex");
    assert!(custom.contains("service tier: `flex`"));
    assert!(custom.contains("来源: 配置"));

    let unresolved = format_external_cli_fast_status(Some("  "), None, "Codex");
    assert!(unresolved.contains("未显式设置 service tier"));
    assert!(unresolved.contains("Codex 自身默认模式"));
}

#[test]
fn codex_adapter_maps_legacy_search_to_web_search_feature() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            search: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--enable", "web_search"));
    assert!(!spec.args.contains(&"--search".to_string()));
}

#[test]
fn codex_adapter_danger_full_access_suppresses_sandbox() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            sandbox: Some("workspace-write".to_string()),
            danger_full_access: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert!(!spec.args.contains(&"--sandbox".to_string()));
}

#[test]
fn codex_adapter_builds_resume_command_from_thread_id() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello again".to_string(),
        operation: default_operation(),
        params: serde_json::json!({ "threadId": "thread-existing" }),
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            profile: Some("not-supported-by-resume".to_string()),
            model: Some("gpt-test".to_string()),
            sandbox: Some("workspace-write".to_string()),
            danger_full_access: Some(true),
            add_dirs: vec!["/tmp/extra".to_string()],
            ephemeral: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert_eq!(spec.executable, "codex");
    assert_eq!(spec.args[0], "exec");
    assert!(spec.args.contains(&"resume".to_string()));
    assert!(has_arg_pair(&spec.args, "--cd", "/tmp/work"));
    assert!(has_arg_pair(&spec.args, "--model", "gpt-test"));
    assert!(spec.args.contains(&"--ephemeral".to_string()));
    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert!(spec.args.contains(&"thread-existing".to_string()));
    assert!(!spec.args.contains(&"--profile".to_string()));
    assert!(!spec.args.contains(&"--sandbox".to_string()));
    assert!(!spec.args.contains(&"--add-dir".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn codex_adapter_injects_work_dir_with_custom_args() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            args: vec!["exec".to_string(), "--json".to_string(), "-".to_string()],
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert_eq!(spec.args[0], "exec");
    assert!(has_arg_pair(&spec.args, "--cd", "/tmp/work"));
    assert!(has_arg_pair(
        &spec.args,
        "--output-last-message",
        "/tmp/last.md"
    ));
    assert_eq!(spec.work_dir.as_deref(), Some(Path::new("/tmp/work")));
}

#[test]
fn codex_adapter_applies_config_flags_to_custom_args() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--model".to_string(),
                "gpt-runner".to_string(),
                "--sandbox".to_string(),
                "workspace-write".to_string(),
                "-".to_string(),
            ],
            model: Some("gpt-schedule".to_string()),
            reasoning_effort: Some("high".to_string()),
            enable_features: vec!["web_search".to_string()],
            danger_full_access: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--cd", "/tmp/work"));
    assert!(has_arg_pair(
        &spec.args,
        "--output-last-message",
        "/tmp/last.md"
    ));
    assert!(has_arg_pair(&spec.args, "--model", "gpt-schedule"));
    assert!(!has_arg_pair(&spec.args, "--model", "gpt-runner"));
    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "model_reasoning_effort=\"high\""
    ));
    assert!(has_arg_pair(&spec.args, "--enable", "web_search"));
    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert!(!spec.args.contains(&"--sandbox".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn claude_code_adapter_applies_session_model_to_command_spec() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some(DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string()),
        session_key: Some("session:claude".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: CLAUDE_CODE_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            model: Some("claude-opus-4-5-20251101".to_string()),
            danger_full_access: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert_eq!(spec.executable, "claude");
    assert!(spec.args.contains(&"-p".to_string()));
    assert!(has_arg_pair(&spec.args, "--input-format", "stream-json"));
    assert!(spec.args.contains(&"--replay-user-messages".to_string()));
    assert!(has_arg_pair(
        &spec.args,
        "--model",
        "claude-opus-4-5-20251101"
    ));
    assert!(spec
        .args
        .contains(&"--dangerously-skip-permissions".to_string()));
}

#[test]
fn claude_code_explicit_exec_transport_keeps_text_stdin() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some(DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string()),
        session_key: Some("session:claude-exec".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: CLAUDE_CODE_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            transport: Some(ExternalCliTransport::Exec),
            danger_full_access: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--input-format", "text"));
    assert!(!spec.args.contains(&"--replay-user-messages".to_string()));
}

#[test]
fn claude_code_adapter_applies_session_effort_to_command_spec() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some(DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string()),
        session_key: Some("session:claude".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: CLAUDE_CODE_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            reasoning_effort: Some("xhigh".to_string()),
            danger_full_access: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--effort", "xhigh"));
}

#[test]
fn traex_adapter_applies_session_effort_to_command_spec() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some(DEFAULT_TRAEX_RUNNER_ID.to_string()),
        session_key: Some("session:traex".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: TRAEX_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            reasoning_effort: Some("high".to_string()),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "model_reasoning_effort=\"high\""
    ));
}

#[tokio::test]
async fn final_response_prefers_assistant_message_over_run_finished() {
    let temp_dir = tempfile::tempdir().unwrap();
    let missing_last_message = temp_dir.path().join("last.md");
    let events = parse_progress_events(
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"real final"}}
{"type":"turn.completed"}"#,
    );

    let response = final_response(&missing_last_message, "raw stdout", &events)
        .await
        .unwrap();

    assert_eq!(response, "real final");
}

#[tokio::test]
async fn final_response_prefers_run_failed_message_over_protocol_stdout() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events = vec![ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::RunFailed,
        content: "request timed out".to_string(),
        title: Some("Codex error".to_string()),
        raw: serde_json::json!({"method":"error"}),
    }];

    let response = final_response(
        &temp_dir.path().join("missing.md"),
        r#"{"id":1,"result":{"userAgent":"Codex Desktop"}}"#,
        &events,
    )
    .await
    .unwrap();

    assert_eq!(response, "request timed out");
}

#[tokio::test]
async fn final_response_falls_back_to_trimmed_stdout() {
    let temp_dir = tempfile::tempdir().unwrap();
    let response = final_response(&temp_dir.path().join("missing.md"), "  raw fallback  ", &[])
        .await
        .unwrap();

    assert_eq!(response, "raw fallback");
}

#[tokio::test]
async fn external_cli_runtime_runs_mock_command_and_writes_artifacts() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runtime = ExternalCliRuntime::new(temp_dir.path());
    let request = ExternalCliRunRequest {
        images: Vec::new(),
            files: Vec::new(),
        message: "hello from api".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("chat-gateway-test".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "mock".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                "cat >/dev/null; printf '%s\n' '{\"type\":\"assistant_delta\",\"delta\":\"working\"}' '{\"type\":\"assistant_final\",\"content\":\"mock final\"}'"
                    .to_string(),
            ],
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let result = runtime.run(request).await.unwrap();

    assert_eq!(result.status, ExternalCliRunStatus::Succeeded);
    assert_eq!(result.response, "mock final");
    assert_eq!(result.events.len(), 2);
    assert!(Path::new(&result.artifacts.command_snapshot).exists());
    assert!(Path::new(&result.artifacts.normalized_events).exists());
    let prompt_summary: serde_json::Value = serde_json::from_str(
        &tokio::fs::read_to_string(&result.artifacts.prompt)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(prompt_summary["_bifrost_compacted"], true);
    assert!(!tokio::fs::read_to_string(&result.artifacts.prompt)
        .await
        .unwrap()
        .contains("hello from api"));
    let persisted: ExternalCliRunResult = serde_json::from_str(
        &tokio::fs::read_to_string(Path::new(&result.artifacts.run_dir).join("result.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(persisted.responses.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn external_cli_runtime_dispatches_default_claude_stream_json_transport() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().unwrap();
    let executable = temp_dir.path().join("mock-claude-runtime");
    std::fs::write(
        &executable,
        r#"#!/usr/bin/env python3
import json
import sys

if "--version" in sys.argv:
    print("mock claude 1.0")
    raise SystemExit(0)

first = json.loads(sys.stdin.readline())
print(json.dumps({"type":"system","subtype":"init","session_id":"runtime-stream-session"}), flush=True)
print(json.dumps(first), flush=True)
print(json.dumps({"type":"assistant","message":{"content":[{"type":"text","text":"runtime stream final"}]},"session_id":"runtime-stream-session"}), flush=True)
print(json.dumps({"type":"result","subtype":"success","is_error":False,"result":"runtime stream final","session_id":"runtime-stream-session"}), flush=True)
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let runtime = ExternalCliRuntime::new(temp_dir.path().join("runs"));
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello stream runtime".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some(DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string()),
        session_key: Some("runtime-stream-session-key".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: CLAUDE_CODE_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some(executable.display().to_string()),
            // This is an integration-style process test that includes executable probing and
            // Python startup. Keep enough scheduling headroom when the workspace suite is busy.
            timeout_secs: Some(60),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    assert!(ExternalCliTransport::AppServer.supports_live_guide());
    assert!(ExternalCliTransport::StreamJson.supports_live_guide());
    assert!(!ExternalCliTransport::Exec.supports_live_guide());
    assert_eq!(
        resolved_transport_name_for_request(&request),
        Some("stream_json")
    );
    let result = runtime.run(request).await.unwrap();
    assert_eq!(result.status, ExternalCliRunStatus::Succeeded);
    assert_eq!(result.response, "runtime stream final");
    assert!(result
        .events
        .iter()
        .any(|event| event.event_type == ExternalCliProgressEventType::AssistantFinal));
}

#[tokio::test]
async fn external_cli_runtime_persists_chatgpt_web_adapter_errors() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runtime = ExternalCliRuntime::new(temp_dir.path());
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello from daily agent".to_string(),
        operation: "unsupported-test-operation".to_string(),
        params: serde_json::Value::Null,
        provider_id: None,
        runner_id: Some("web".to_string()),
        session_key: None,
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            timeout_secs: Some(1),
            extra: BTreeMap::from([("browser".to_string(), serde_json::json!("invalid"))]),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let result = runtime.run(request).await.unwrap();

    assert_eq!(result.status, ExternalCliRunStatus::Failed);
    assert!(result.response.contains("ChatGPT Web run failed"));
    assert!(result
        .response
        .contains("parse chatgpt_web adapter config failed"));
    assert!(Path::new(&result.artifacts.command_snapshot).exists());
    assert!(Path::new(&result.artifacts.stdout).exists());
    assert!(Path::new(&result.artifacts.stderr).exists());
    assert!(Path::new(&result.artifacts.normalized_events).exists());
    assert!(!Path::new(&result.artifacts.last_message).exists());
    let stderr = tokio::fs::read_to_string(&result.artifacts.stderr)
        .await
        .unwrap();
    assert!(stderr.contains("parse chatgpt_web adapter config failed"));
    assert!(Path::new(&result.artifacts.run_dir)
        .join("result.json")
        .exists());
    assert!(result.metadata.contains_key("failureDiagnostics"));
    assert_eq!(
        result.events[0].event_type,
        ExternalCliProgressEventType::RunFailed
    );
}

#[tokio::test]
async fn external_cli_runtime_streams_stdout_before_process_exit() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runtime = ExternalCliRuntime::new(temp_dir.path());
    let request = ExternalCliRunRequest {
        images: Vec::new(),
            files: Vec::new(),
        message: "hello stream".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("chat-gateway-stream-test".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "mock".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                "cat >/dev/null; printf '%s\n' '{\"type\":\"assistant_delta\",\"delta\":\"streaming now\"}'; sleep 1; printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"stream final\"}'".to_string(),
            ],
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let (progress_tx, mut progress_rx) =
        tokio::sync::mpsc::channel(EXTERNAL_CLI_PROGRESS_CHANNEL_CAPACITY);
    let run = tokio::spawn(async move {
        runtime
            .run_with_progress(request, Some(progress_tx))
            .await
            .unwrap()
    });

    let first = tokio::time::timeout(Duration::from_secs(10), progress_rx.recv())
        .await
        .expect("progress event should arrive before process exit")
        .expect("progress channel open");

    assert_eq!(
        first.event_type,
        ExternalCliProgressEventType::AssistantDelta
    );
    assert_eq!(first.content, "streaming now");
    assert!(
        !run.is_finished(),
        "mock command sleeps after first event, so runtime must still be active"
    );
    let result = run.await.unwrap();
    assert_eq!(result.response, "stream final");
    assert_eq!(result.events.len(), 2);
}

#[test]
fn external_progress_maps_to_agent_turn_progress_events() {
    let event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::AssistantDelta,
        content: "thinking out loud".to_string(),
        title: None,
        raw: serde_json::json!({"type":"assistant_delta","delta":"thinking out loud"}),
    };

    let mapped = external_progress_to_agent_turn_event(
        "session-a",
        TRAEX_ADAPTER,
        ExternalCliProgressStatusContext::new(
            Some("traex"),
            Some("trae-model"),
            Some("runner config"),
            Some("high"),
            Some("auto"),
            Some(Path::new("/tmp/work")),
        ),
        &event,
    )
    .expect("mapped event");

    match mapped {
        bifrost_agent::AgentTurnProgressEvent::AssistantDelta { content } => {
            assert_eq!(content, "thinking out loud");
        }
        other => panic!("unexpected mapped event: {other:?}"),
    }

    let status_event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::Status,
        content: "running".to_string(),
        title: None,
        raw: serde_json::json!({"type":"status","content":"running"}),
    };
    let mapped_status = external_progress_to_agent_turn_event(
        "session-a",
        TRAEX_ADAPTER,
        ExternalCliProgressStatusContext::new(
            Some("traex"),
            Some("trae-model"),
            Some("runner config"),
            Some("high"),
            Some("auto"),
            Some(Path::new("/tmp/work")),
        ),
        &status_event,
    )
    .expect("mapped status event");
    match mapped_status {
        bifrost_agent::AgentTurnProgressEvent::Status(status) => {
            assert_eq!(status.runner_type.as_deref(), Some(TRAEX_ADAPTER));
            assert_eq!(status.runner_id.as_deref(), Some("traex"));
            assert_eq!(status.model.as_deref(), Some("trae-model"));
            assert_eq!(status.model_provider.as_deref(), Some("runner config"));
            assert_eq!(status.model_reasoning_effort.as_deref(), Some("high"));
            assert_eq!(status.model_reasoning_summary.as_deref(), Some("auto"));
        }
        other => panic!("unexpected mapped status event: {other:?}"),
    }

    let plan_event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::PlanUpdated,
        content: "plan updated (2 steps)".to_string(),
        title: None,
        raw: serde_json::json!({
            "type": "item.updated",
            "item": {
                "type": "todo_list",
                "items": [
                    {"text": "inspect output", "completed": true},
                    {"text": "map parser", "completed": false}
                ]
            }
        }),
    };
    let mapped_plan = external_progress_to_agent_turn_event(
        "session-a",
        TRAEX_ADAPTER,
        ExternalCliProgressStatusContext::new(
            Some("traex"),
            Some("trae-model"),
            Some("runner config"),
            Some("high"),
            Some("auto"),
            Some(Path::new("/tmp/work")),
        ),
        &plan_event,
    )
    .expect("mapped plan event");
    match mapped_plan {
        bifrost_agent::AgentTurnProgressEvent::PlanUpdated { steps, title } => {
            assert!(title.is_none());
            assert_eq!(steps.len(), 2);
            assert_eq!(steps[0].status, PlanStepStatus::Completed);
            assert_eq!(steps[1].status, PlanStepStatus::Pending);
        }
        other => panic!("unexpected mapped plan event: {other:?}"),
    }
}

#[tokio::test]
async fn external_cli_run_writes_attachments_and_injects_prompt_paths() {
    let temp_dir = tempfile::tempdir().unwrap();
    let _data_dir_guard = crate::test_env::BifrostDataDirGuard::set(temp_dir.path());
    let runs_root = temp_dir.path().join("runs");
    let runtime = ExternalCliRuntime::new(&runs_root);
    let request = ExternalCliRunRequest {
        images: vec![
            ExternalCliImageInput {
                mime_type: "image/png".to_string(),
                data: "aGVsbG8=".to_string(),
                name: Some("pasted.png".to_string()),
            },
            ExternalCliImageInput {
                mime_type: "image/jpeg".to_string(),
                data: "dHdv".to_string(),
                name: Some("second.jpg".to_string()),
            },
        ],
        files: Vec::new(),
        message: String::new(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("chat-gateway-image-test".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "mock".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                "cat >/dev/null; printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"saw image\"}'".to_string(),
            ],
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let mut file_request = request.clone();
    file_request.images = Vec::new();
    file_request.files = vec![ExternalCliFileInput {
        mime_type: "text/plain".to_string(),
        data: "cmVwb3J0IGJvZHk=".to_string(),
        name: Some("../report final.md".to_string()),
    }];
    let mut second_request = request.clone();
    second_request.images = vec![ExternalCliImageInput {
        mime_type: "image/png".to_string(),
        data: "d29ybGQ=".to_string(),
        name: Some("second.png".to_string()),
    }];

    let result = runtime.run(request).await.unwrap();

    let prompt_summary: serde_json::Value = serde_json::from_str(
        &tokio::fs::read_to_string(&result.artifacts.prompt)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(prompt_summary["_bifrost_compacted"], true);
    assert_eq!(prompt_summary["image_count"], 2);
    let images: Vec<ExternalCliSavedImageAttachment> = serde_json::from_str(
        result
            .metadata
            .get("attachments.images")
            .expect("attachments metadata"),
    )
    .unwrap();
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].mime_type, "image/png");
    assert_eq!(images[1].mime_type, "image/jpeg");
    let first_image_path = std::path::PathBuf::from(&images[0].path);
    let second_saved_image_path = std::path::PathBuf::from(&images[1].path);
    assert_eq!(
        first_image_path.parent(),
        Some(
            runs_root
                .join(&result.run_id)
                .join("attachments")
                .join("images")
                .as_path()
        )
    );
    assert_eq!(
        first_image_path.file_name().and_then(|v| v.to_str()),
        Some("image-1.png")
    );
    assert_eq!(
        second_saved_image_path.file_name().and_then(|v| v.to_str()),
        Some("image-2.jpg")
    );
    assert_eq!(tokio::fs::read(&images[0].path).await.unwrap(), b"hello");
    assert_eq!(tokio::fs::read(&images[1].path).await.unwrap(), b"two");

    let file_result = runtime.run(file_request).await.unwrap();
    let file_prompt_summary: serde_json::Value = serde_json::from_str(
        &tokio::fs::read_to_string(&file_result.artifacts.prompt)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(file_prompt_summary["_bifrost_compacted"], true);
    assert_eq!(file_prompt_summary["file_count"], 1);
    let files: Vec<ExternalCliSavedFileAttachment> = serde_json::from_str(
        file_result
            .metadata
            .get("attachments.files")
            .expect("file attachments metadata"),
    )
    .unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].mime_type, "text/plain");
    assert_eq!(files[0].name.as_deref(), Some("../report final.md"));
    let file_path = std::path::PathBuf::from(&files[0].path);
    assert_eq!(
        file_path.parent(),
        Some(
            runs_root
                .join(&file_result.run_id)
                .join("attachments")
                .join("files")
                .as_path()
        )
    );
    assert_eq!(
        file_path.file_name().and_then(|v| v.to_str()),
        Some("1-report_final.md")
    );
    assert_eq!(
        tokio::fs::read(&files[0].path).await.unwrap(),
        b"report body"
    );
    assert_eq!(
        file_result.metadata.get("attachments.fileCount"),
        Some(&"1".to_string())
    );

    let second_result = runtime.run(second_request).await.unwrap();
    let second_images: Vec<ExternalCliSavedImageAttachment> = serde_json::from_str(
        second_result
            .metadata
            .get("attachments.images")
            .expect("attachments metadata"),
    )
    .unwrap();
    assert_eq!(second_images.len(), 1);
    let second_image_path = std::path::PathBuf::from(&second_images[0].path);
    assert_ne!(first_image_path, second_image_path);
    assert_eq!(
        second_image_path.parent(),
        Some(
            runs_root
                .join(&second_result.run_id)
                .join("attachments")
                .join("images")
                .as_path()
        )
    );
    assert_eq!(
        tokio::fs::read(&first_image_path).await.unwrap(),
        b"hello",
        "second run must not overwrite first run attachment"
    );
    assert_eq!(tokio::fs::read(&second_image_path).await.unwrap(), b"world");
}

#[tokio::test]
async fn live_guide_prompt_persists_session_images_and_rejects_unsafe_ids() {
    let temp_dir = tempfile::tempdir().unwrap();
    let _data_dir_guard = crate::test_env::BifrostDataDirGuard::set(temp_dir.path());
    let session_key = "im:feishu-main:live-guide-image";
    let guide_id = "guide-image-test";
    let prompt = prepare_live_guide_prompt(
        session_key,
        guide_id,
        "inspect the screenshot and focus on the error",
        &[bifrost_agent::ChatImageInput {
            mime_type: "image/png".to_string(),
            data: "aW1hZ2UtYnl0ZXM=".to_string(),
        }],
        &[ExternalCliFileInput {
            mime_type: "text/plain".to_string(),
            data: "bG9nLWJ5dGVz".to_string(),
            name: Some("runtime.log".to_string()),
        }],
    )
    .await
    .expect("prepare live guide prompt");

    let history_path = bifrost_agent::persistence::canonical_conversation_path(
        &bifrost_agent::config::agent_home_dir(),
        session_key,
    );
    let attachment_dir = history_path
        .parent()
        .unwrap()
        .join("attachments")
        .join(history_path.file_stem().unwrap())
        .join(guide_id);
    let image_path = attachment_dir.join("images").join("image-1.png");
    let file_path = attachment_dir.join("files").join("1-runtime.log");
    assert_eq!(tokio::fs::read(&image_path).await.unwrap(), b"image-bytes");
    assert_eq!(tokio::fs::read(&file_path).await.unwrap(), b"log-bytes");
    assert!(prompt.contains("## Attached Images"), "{prompt}");
    assert!(prompt.contains("## Attached Files"), "{prompt}");
    assert!(
        prompt.contains(&image_path.display().to_string()),
        "{prompt}"
    );
    assert!(
        prompt.contains(&file_path.display().to_string()),
        "{prompt}"
    );
    assert!(
        prompt.ends_with("inspect the screenshot and focus on the error\n"),
        "{prompt}"
    );

    let error = prepare_live_guide_prompt(session_key, "../escape", "unsafe", &[], &[])
        .await
        .expect_err("unsafe guide id must be rejected");
    assert!(error.contains("safe path component"), "{error}");
    assert!(prepare_live_guide_prompt("  ", "guide", "unsafe", &[], &[])
        .await
        .unwrap_err()
        .contains("session_key cannot be empty"));
}

#[test]
fn worker_bootstrap_parsers_cover_missing_invalid_and_valid_inputs() {
    let _guard = external_cli_env_guard();
    let _protocol = EnvGuard::unset(EXTERNAL_CLI_WORKER_BOOTSTRAP_PROTOCOL_ENV);
    let _runs = EnvGuard::unset(EXTERNAL_CLI_WORKER_BOOTSTRAP_RUNS_ROOT_ENV);
    let _request = EnvGuard::unset(EXTERNAL_CLI_WORKER_BOOTSTRAP_REQUEST_PATH_ENV);
    assert!(external_cli_worker_bootstrap_from_environment()
        .unwrap()
        .is_none());

    {
        let _runs = EnvGuard::set_str(EXTERNAL_CLI_WORKER_BOOTSTRAP_RUNS_ROOT_ENV, "/runs");
        assert!(external_cli_worker_bootstrap_from_environment()
            .unwrap_err()
            .contains(EXTERNAL_CLI_WORKER_BOOTSTRAP_PROTOCOL_ENV));
    }
    {
        let _protocol = EnvGuard::set_str(EXTERNAL_CLI_WORKER_BOOTSTRAP_PROTOCOL_ENV, "bad");
        assert!(external_cli_worker_bootstrap_from_environment()
            .unwrap_err()
            .contains("parse external runner worker bootstrap protocol"));
    }
    {
        let _protocol = EnvGuard::set_str(EXTERNAL_CLI_WORKER_BOOTSTRAP_PROTOCOL_ENV, "1");
        assert!(external_cli_worker_bootstrap_from_environment()
            .unwrap_err()
            .contains(EXTERNAL_CLI_WORKER_BOOTSTRAP_RUNS_ROOT_ENV));
        let _runs = EnvGuard::set_str(EXTERNAL_CLI_WORKER_BOOTSTRAP_RUNS_ROOT_ENV, "/runs");
        assert!(external_cli_worker_bootstrap_from_environment()
            .unwrap_err()
            .contains(EXTERNAL_CLI_WORKER_BOOTSTRAP_REQUEST_PATH_ENV));
        let _request = EnvGuard::set_str(
            EXTERNAL_CLI_WORKER_BOOTSTRAP_REQUEST_PATH_ENV,
            "/request.json",
        );
        let parsed = external_cli_worker_bootstrap_from_environment()
            .unwrap()
            .unwrap();
        assert_eq!(parsed.protocol_version, 1);
        assert_eq!(parsed.runs_root, "/runs");
        assert_eq!(parsed.request_path, PathBuf::from("/request.json"));
    }

    let mut empty = std::io::Cursor::new(Vec::<u8>::new());
    assert!(read_external_cli_worker_stdin_bootstrap(&mut empty)
        .unwrap_err()
        .contains("expected a run command"));
    let mut wrong = std::io::Cursor::new(b"{\"type\":\"stop\"}\n".to_vec());
    assert!(read_external_cli_worker_stdin_bootstrap(&mut wrong)
        .unwrap_err()
        .contains("first command must be run"));
    let mut valid = std::io::Cursor::new(
        b"{\"type\":\"run\",\"request\":{\"protocolVersion\":1,\"runsRoot\":\"/runs\",\"requestPath\":\"/request.json\"}}\n".to_vec(),
    );
    assert_eq!(
        read_external_cli_worker_stdin_bootstrap(&mut valid)
            .unwrap()
            .runs_root,
        "/runs"
    );
}

#[tokio::test]
async fn external_cli_file_attachments_cover_limits_base_dir_and_name_edges() {
    let temp_dir = tempfile::tempdir().unwrap();
    let data_dir = temp_dir.path().join("data");
    let _data_dir_guard = crate::test_env::BifrostDataDirGuard::set(&data_dir);
    let runs_root = temp_dir.path().join("runs");
    let runtime = ExternalCliRuntime::new(&runs_root);
    let attachment_base = bifrost_agent::config::agent_home_dir()
        .join("sessions")
        .join("by-key")
        .join("attachments")
        .join("session-edge");
    let long_name = format!("{}.txt", "a".repeat(220));
    let mut files = vec![
        ExternalCliFileInput {
            mime_type: "text/plain".to_string(),
            data: "YWxwaGE=".to_string(),
            name: Some("......".to_string()),
        },
        ExternalCliFileInput {
            mime_type: "text/plain".to_string(),
            data: "YmV0YQ==".to_string(),
            name: Some("duplicate.txt".to_string()),
        },
        ExternalCliFileInput {
            mime_type: "text/plain".to_string(),
            data: "Z2FtbWE=".to_string(),
            name: Some("duplicate.txt".to_string()),
        },
        ExternalCliFileInput {
            mime_type: "text/plain".to_string(),
            data: "ZGVsdGE=".to_string(),
            name: Some(long_name),
        },
        ExternalCliFileInput {
            mime_type: "text/plain".to_string(),
            data: "ZW1wdHk=".to_string(),
            name: Some(String::new()),
        },
        ExternalCliFileInput {
            mime_type: "text/plain".to_string(),
            data: "emV0YQ==".to_string(),
            name: Some("six.bin".to_string()),
        },
    ];
    files.push(ExternalCliFileInput {
        mime_type: "text/plain".to_string(),
        data: "dHJ1bmNhdGVk".to_string(),
        name: Some("seven.bin".to_string()),
    });
    files.push(ExternalCliFileInput {
        mime_type: "text/plain".to_string(),
        data: "   ".to_string(),
        name: Some("blank.bin".to_string()),
    });
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files,
        message: "check attached files".to_string(),
        operation: default_operation(),
        params: serde_json::json!({
            "attachmentBaseDir": attachment_base,
        }),
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("file-edge-session".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "mock".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                "cat >/dev/null; printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"ok\"}'"
                    .to_string(),
            ],
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let result = runtime.run(request).await.unwrap();

    let saved: Vec<ExternalCliSavedFileAttachment> = serde_json::from_str(
        result
            .metadata
            .get("attachments.files")
            .expect("file attachment metadata"),
    )
    .unwrap();
    assert_eq!(saved.len(), MAX_EXTERNAL_RUNNER_ATTACHMENTS_PER_MESSAGE);
    assert!(saved
        .iter()
        .all(|file| std::path::Path::new(&file.path).starts_with(&attachment_base)));
    assert_eq!(
        saved[0].path.as_str(),
        attachment_base
            .join(&result.run_id)
            .join("files")
            .join("1-attachment.bin")
            .display()
            .to_string()
    );
    assert_eq!(
        std::path::Path::new(&saved[4].path)
            .file_name()
            .and_then(|value| value.to_str()),
        Some("5-attachment.bin")
    );
    assert!(std::path::Path::new(&saved[3].path)
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.len() <= 162 && name.starts_with("4-")));
    assert_eq!(
        result.metadata.get("attachments.fileCount"),
        Some(&MAX_EXTERNAL_RUNNER_ATTACHMENTS_PER_MESSAGE.to_string())
    );
}

#[tokio::test]
async fn external_cli_file_attachment_collision_uses_uuid_fallback_name() {
    let temp_dir = tempfile::tempdir().unwrap();
    let files_dir = temp_dir.path().join("files");
    tokio::fs::create_dir_all(&files_dir).await.unwrap();
    tokio::fs::write(files_dir.join("1-existing.txt"), b"old")
        .await
        .unwrap();

    let path = unique_file_attachment_path(&files_dir, 1, "existing.txt").await;

    let file_name = path.file_name().and_then(|value| value.to_str()).unwrap();
    assert_ne!(file_name, "1-existing.txt");
    assert!(file_name.starts_with("1-"));
    assert_eq!(path.parent(), Some(files_dir.as_path()));
}

#[test]
fn external_cli_file_input_defaults_mime_type() {
    let file: ExternalCliFileInput =
        serde_json::from_value(serde_json::json!({ "data": "YWJj" })).unwrap();

    assert_eq!(file.mime_type, "application/octet-stream");
    assert_eq!(file.data, "YWJj");
}

#[tokio::test]
async fn external_cli_runtime_marks_stopped_run_before_late_stdout() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runs_root = temp_dir.path().to_path_buf();
    let runtime = ExternalCliRuntime::new(&runs_root);
    let (executable, args) = delayed_final_command("too late");
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "stop me".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("stop-test".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "mock".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some(executable),
            args,
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let handle = tokio::spawn(async move { runtime.run(request).await.unwrap() });
    let run_id = wait_for_single_run_dir(&runs_root).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    request_run_stop(&runs_root, &run_id).await.unwrap();

    let result = handle.await.unwrap();

    assert_eq!(result.run_id, run_id);
    assert_eq!(result.status, ExternalCliRunStatus::Stopped);
    assert_eq!(result.response, "External CLI run was stopped by request.");
    assert_eq!(result.exit_code, None);
    assert_eq!(
        result.events[0].event_type,
        ExternalCliProgressEventType::RunFailed
    );
}

#[tokio::test]
async fn read_run_detail_prefers_persisted_result_response_over_stdout() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runs_root = temp_dir.path().to_path_buf();
    let run_id = "detail-response-run";
    let run_dir = runs_root.join(run_id);
    tokio::fs::create_dir_all(&run_dir).await.unwrap();
    tokio::fs::write(run_dir.join("runtime_snapshot.json"), "{}")
        .await
        .unwrap();
    tokio::fs::write(
        run_dir.join("normalized_events.jsonl"),
        r#"{"eventType":"run_failed","content":"External CLI run was stopped by request.","title":"Stopped","raw":{"type":"run_stopped"}}"#,
    )
    .await
    .unwrap();
    tokio::fs::write(run_dir.join("cli.stdout.log"), "raw streaming stdout\n")
        .await
        .unwrap();
    tokio::fs::write(
        run_dir.join("cli.stderr.log"),
        "external cli stopped by request\n",
    )
    .await
    .unwrap();
    let result = ExternalCliRunResult {
        run_id: run_id.to_string(),
        session_key: Some("detail-response-session".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "traex".to_string(),
        status: ExternalCliRunStatus::Stopped,
        exit_code: None,
        response: "External CLI run was stopped by request.".to_string(),
        responses: vec!["External CLI run was stopped by request.".to_string()],
        started_at: 1,
        finished_at: 2,
        duration_ms: 1,
        artifacts: ExternalCliRunArtifacts {
            run_dir: run_dir.display().to_string(),
            prompt: run_dir.join("prompt.md").display().to_string(),
            command_snapshot: run_dir.join("runtime_snapshot.json").display().to_string(),
            stdout: run_dir.join("cli.stdout.log").display().to_string(),
            stderr: run_dir.join("cli.stderr.log").display().to_string(),
            normalized_events: run_dir
                .join("normalized_events.jsonl")
                .display()
                .to_string(),
            last_message: run_dir.join("last_message.md").display().to_string(),
        },
        events: Vec::new(),
        metadata: std::collections::BTreeMap::new(),
    };
    tokio::fs::write(
        run_dir.join("result.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .await
    .unwrap();

    let detail = read_run_detail(&runs_root, run_id).await.unwrap();

    assert_eq!(detail.response, "External CLI run was stopped by request.");
}

#[test]
fn visible_terminal_response_uses_stderr_for_empty_failed_result() {
    let response = visible_terminal_response(
        ExternalCliRunStatus::Failed,
        String::new(),
        "",
        "Error loading config.toml: unknown variant `default`, expected `fast` or `flex`\n",
        &[],
    );

    assert_eq!(
        response,
        "Error loading config.toml: unknown variant `default`, expected `fast` or `flex`"
    );
}

#[tokio::test]
async fn external_cli_runtime_stops_active_run_by_session_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runs_root = temp_dir.path().to_path_buf();
    let runtime = ExternalCliRuntime::new(&runs_root);
    let (executable, args) = delayed_final_command("too late");
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "stop by session".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("im:provider-a:user-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "mock".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some(executable),
            args,
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let handle = tokio::spawn(async move { runtime.run(request).await.unwrap() });
    let run_id = wait_for_single_run_dir(&runs_root).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    request_session_stop(&runs_root, "im:provider-a:user-a")
        .await
        .unwrap();

    let result = handle.await.unwrap();

    assert_eq!(result.run_id, run_id);
    assert_eq!(result.status, ExternalCliRunStatus::Stopped);
    assert_eq!(result.response, "External CLI run was stopped by request.");
}

#[cfg(unix)]
#[tokio::test]
async fn app_server_session_stop_sends_interrupt_and_preserves_real_run_result() {
    let _registry_guard = external_cli_env_guard_async().await;
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().unwrap();
    let runs_root = temp_dir.path().join("runs");
    let executable = temp_dir.path().join("mock-codex-stop.py");
    let protocol_log = temp_dir.path().join("app-server-protocol.jsonl");
    std::fs::write(
        &executable,
        r#"#!/usr/bin/env python3
import json
import os
import sys

log_path = os.environ["BIFROST_STOP_PROTOCOL_LOG"]

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

for line in sys.stdin:
    frame = json.loads(line)
    with open(log_path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(frame, separators=(",", ":")) + "\n")
    method = frame.get("method")
    request_id = frame.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method == "thread/start":
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":"thread-stop"}}})
    elif method == "turn/start":
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":"turn-stop"}}})
    elif method == "account/rateLimits/read":
        send({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"unsupported"}})
    elif method == "turn/interrupt":
        send({"jsonrpc":"2.0","id":request_id,"result":{}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"thread-stop","turn":{"id":"turn-stop","status":"interrupted"}}})
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let session_key = format!("app-server-stop-{}", uuid::Uuid::new_v4());
    let mut env = BTreeMap::new();
    env.insert(
        "BIFROST_STOP_PROTOCOL_LOG".to_string(),
        protocol_log.display().to_string(),
    );
    let request = ExternalCliRunRequest {
        message: "wait until stopped".to_string(),
        images: Vec::new(),
        files: Vec::new(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-stop".to_string()),
        runner_id: Some("codex-stop".to_string()),
        session_key: Some(session_key.clone()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some(executable.display().to_string()),
            transport: Some(ExternalCliTransport::AppServer),
            env,
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let runtime = ExternalCliRuntime::new(&runs_root);
    let run = tokio::spawn(async move { runtime.run(request).await.unwrap() });
    wait_for_file_text(&protocol_log, "turn/start").await;

    request_session_stop(&runs_root, &session_key)
        .await
        .expect("request app-server stop");
    let result = timeout(Duration::from_secs(5), run)
        .await
        .expect("app-server stop should finish")
        .expect("join app-server stop");

    assert_eq!(result.status, ExternalCliRunStatus::Stopped);
    assert!(!result.run_id.starts_with("stopped-"), "{result:?}");
    assert!(!result.artifacts.run_dir.is_empty(), "{result:?}");
    let log = tokio::fs::read_to_string(&protocol_log).await.unwrap();
    assert!(log.contains("turn/interrupt"), "{log}");
    assert!(log.contains("thread-stop"), "{log}");
    assert!(log.contains("turn-stop"), "{log}");
}

#[cfg(unix)]
#[tokio::test]
async fn claude_stream_json_session_stop_sends_interrupt_control_frame() {
    let _registry_guard = external_cli_env_guard_async().await;
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().unwrap();
    let runs_root = temp_dir.path().join("runs");
    let executable = temp_dir.path().join("mock-claude-stop.py");
    let protocol_log = temp_dir.path().join("claude-protocol.jsonl");
    std::fs::write(
        &executable,
        r#"#!/usr/bin/env python3
import json
import os
import sys

if "--version" in sys.argv:
    print("2.1.0")
    raise SystemExit(0)

log_path = os.environ["BIFROST_STOP_PROTOCOL_LOG"]
for line in sys.stdin:
    frame = json.loads(line)
    with open(log_path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(frame, separators=(",", ":")) + "\n")
    if frame.get("type") == "user":
        replay = dict(frame)
        replay["session_id"] = "claude-stop-session"
        print(json.dumps(replay, separators=(",", ":")), flush=True)
    elif frame.get("type") == "control_request":
        request_id = frame["request_id"]
        print(json.dumps({"type":"control_response","response":{"subtype":"success","request_id":request_id,"response":{}}}, separators=(",", ":")), flush=True)
        print(json.dumps({"type":"result","subtype":"error_during_execution","is_error":True,"result":"interrupted","session_id":"claude-stop-session"}, separators=(",", ":")), flush=True)
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let session_key = format!("claude-stop-{}", uuid::Uuid::new_v4());
    let mut env = BTreeMap::new();
    env.insert(
        "BIFROST_STOP_PROTOCOL_LOG".to_string(),
        protocol_log.display().to_string(),
    );
    let request = ExternalCliRunRequest {
        message: "wait until stopped".to_string(),
        images: Vec::new(),
        files: Vec::new(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-stop".to_string()),
        runner_id: Some("claude-stop".to_string()),
        session_key: Some(session_key.clone()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: CLAUDE_CODE_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some(executable.display().to_string()),
            transport: Some(ExternalCliTransport::StreamJson),
            env,
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let runtime = ExternalCliRuntime::new(&runs_root);
    let run = tokio::spawn(async move { runtime.run(request).await.unwrap() });
    wait_for_file_text(&protocol_log, "\"type\":\"user\"").await;

    request_session_stop(&runs_root, &session_key)
        .await
        .expect("request Claude stop");
    let result = timeout(Duration::from_secs(5), run)
        .await
        .expect("Claude stop should finish")
        .expect("join Claude stop");

    assert_eq!(result.status, ExternalCliRunStatus::Stopped);
    let log = tokio::fs::read_to_string(&protocol_log).await.unwrap();
    assert!(log.contains("control_request"), "{log}");
    assert!(log.contains("interrupt"), "{log}");
}

#[tokio::test]
async fn worker_guide_rejects_saturated_control_channel_without_waiting() {
    let _registry_guard = external_cli_env_guard_async().await;
    let session_key = format!("saturated-guide-session-{}", uuid::Uuid::new_v4());
    let (guide_tx, _guide_rx) = tokio::sync::mpsc::channel(1);
    let (guide_ack_tx, _guide_ack_rx) = oneshot::channel();
    guide_tx
        .try_send(ExternalCliWorkerGuideRequest {
            guide_id: "fill-guide-channel".to_string(),
            message: "fill".to_string(),
            ack_tx: guide_ack_tx,
        })
        .expect("fill guide channel");
    let (stop_tx, _stop_rx) = tokio::sync::mpsc::unbounded_channel();
    ACTIVE_WORKER_SESSIONS.insert(
        session_key.clone(),
        ExternalCliWorkerControlHandle {
            pid: 1,
            stop_tx,
            guide_tx,
            model_tx: unused_model_tx(),
        },
    );

    let error = request_worker_session_guide(
        &session_key,
        "guide-over-capacity".to_string(),
        "do not wait for a saturated worker".to_string(),
    )
    .await
    .expect_err("saturated guide should fail fast");

    assert!(error.contains("too many pending guide requests"));
    ACTIVE_WORKER_SESSIONS.remove(&session_key);
}

#[tokio::test]
async fn worker_stop_bypasses_saturated_guide_channel() {
    let session_key = format!("saturated-stop-session-{}", uuid::Uuid::new_v4());
    let (guide_tx, _guide_rx) = tokio::sync::mpsc::channel(1);
    let (guide_ack_tx, _guide_ack_rx) = oneshot::channel();
    guide_tx
        .try_send(ExternalCliWorkerGuideRequest {
            guide_id: "fill-guide-channel".to_string(),
            message: "fill".to_string(),
            ack_tx: guide_ack_tx,
        })
        .expect("fill guide channel");
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel();
    ACTIVE_WORKER_SESSIONS.insert(
        session_key.clone(),
        ExternalCliWorkerControlHandle {
            pid: 1,
            stop_tx,
            guide_tx,
            model_tx: unused_model_tx(),
        },
    );
    let ack = tokio::spawn(async move {
        let ack_tx = stop_rx.recv().await.expect("priority stop request");
        let _ = ack_tx.send(());
    });

    assert!(request_worker_session_stop(&session_key).await);
    ack.await.expect("join priority stop acknowledgement");
}

#[test]
fn stale_run_session_cleanup_preserves_replacement_owner() {
    let session_key = format!("replacement-session-{}", uuid::Uuid::new_v4());
    let old_run_id = format!("old-run-{}", uuid::Uuid::new_v4());
    let new_run_id = format!("new-run-{}", uuid::Uuid::new_v4());
    ACTIVE_SESSIONS.insert(session_key.clone(), new_run_id.clone());

    assert!(!remove_active_session_if_owned(&session_key, &old_run_id));
    assert_eq!(
        ACTIVE_SESSIONS
            .get(&session_key)
            .map(|entry| entry.value().clone()),
        Some(new_run_id.clone())
    );

    assert!(remove_active_session_if_owned(&session_key, &new_run_id));
    assert!(!ACTIVE_SESSIONS.contains_key(&session_key));
}

#[test]
fn terminate_process_rejects_pid_zero() {
    let error = terminate_process(0).unwrap_err();

    assert_eq!(error, "refusing to terminate pid 0");
}

#[tokio::test]
async fn request_run_stop_treats_missing_active_pid_as_stopped() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runs_root = temp_dir.path().to_path_buf();
    let run_id = "missing-active-pid-stop";
    tokio::fs::create_dir_all(runs_root.join(run_id))
        .await
        .unwrap();
    ACTIVE_RUNS.insert(run_id.to_string(), 999_999_999);

    request_run_stop(&runs_root, run_id).await.unwrap();

    assert!(
        tokio::fs::try_exists(runs_root.join(run_id).join("stop_requested"))
            .await
            .unwrap()
    );
    tokio::time::sleep(Duration::from_millis(WORKER_TRANSPORT_STOP_GRACE_MS + 100)).await;
    assert!(
        ACTIVE_RUNS.get(run_id).is_none(),
        "missing active pid should be removed by the bounded stop fallback"
    );
}

#[tokio::test]
async fn request_run_stop_does_not_touch_replaced_active_run_owner() {
    let _registry_guard = external_cli_env_guard_async().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let run_id = format!("released-stop-owner-{}", uuid::Uuid::new_v4());
    tokio::fs::create_dir_all(temp_dir.path().join(&run_id))
        .await
        .unwrap();
    ACTIVE_RUNS.insert(run_id.clone(), 999_999_997);

    request_run_stop(temp_dir.path(), &run_id).await.unwrap();
    ACTIVE_RUNS.insert(run_id.clone(), 999_999_996);
    tokio::time::sleep(Duration::from_millis(WORKER_TRANSPORT_STOP_GRACE_MS + 100)).await;

    assert_eq!(
        ACTIVE_RUNS.get(&run_id).map(|entry| *entry.value()),
        Some(999_999_996)
    );
    ACTIVE_RUNS.remove(&run_id);
}

#[test]
fn taskkill_missing_process_messages_are_idempotent() {
    assert!(taskkill_message_indicates_missing_process(
        b"ERROR: The process \"999999999\" not found.",
        b""
    ));
    assert!(taskkill_message_indicates_missing_process(
        b"",
        b"ERROR: The process with PID 999999999 could not be terminated.\r\nReason: There is no running instance of the task.\r\n"
    ));
    assert!(!taskkill_message_indicates_missing_process(
        b"",
        b"ERROR: The process with PID 999999999 could not be terminated.\r\nReason: Access is denied.\r\n"
    ));
}

#[test]
fn effective_config_marks_channel_overrides() {
    let mut config = ExternalCliGatewayConfig::default();
    let runner = config
        .runners
        .get_mut(DEFAULT_CODEX_RUNNER_ID)
        .expect("Codex runner");
    runner.enabled = true;
    runner.adapter = "codex".to_string();
    runner.inject_bifrost_tools = true;
    config.runners.insert(
        "mock-runner".to_string(),
        ExternalCliAgentSettings {
            enabled: true,
            adapter: "mock".to_string(),
            inject_bifrost_tools: false,
            ..Default::default()
        },
    );
    config.channels.insert(
        "feishu-main".to_string(),
        ExternalCliChannelSettings {
            runner_id: Some("mock-runner".to_string()),
            ..Default::default()
        },
    );

    let effective = effective_config_for_provider(&config, Some("feishu-main"));

    assert!(effective.settings.enabled);
    assert_eq!(effective.settings.adapter, "mock");
    assert!(!effective.settings.inject_bifrost_tools);
    assert_eq!(
        effective.sources.get("runnerId").map(String::as_str),
        Some("channel")
    );
    assert_eq!(effective.runner_id, "mock-runner");
}

#[tokio::test]
async fn build_prompt_does_not_inject_legacy_bifrost_tool_context() {
    let settings = ExternalCliAgentSettings {
        enabled: true,
        inject_bifrost_tools: true,
        ..Default::default()
    };
    let request = run_request_from_settings("channel message", None, None, &settings);

    assert!(!request.inject_bifrost_tools);
    let prompt = build_prompt(&request, &[], &[]).await.unwrap();

    assert_eq!(prompt, "channel message\n");
    assert!(!prompt.contains("Bifrost Tool Context"));
}

#[test]
fn compose_message_instructions_uses_base_only_for_new_session() {
    let first_turn = compose_external_cli_message_instructions(
        true,
        Some(" base "),
        Some("developer"),
        Some("user"),
        Some("runner"),
    );
    let resumed_turn = compose_external_cli_message_instructions(
        false,
        Some("base"),
        Some("developer"),
        Some("user"),
        Some("runner"),
    );

    assert_eq!(
        first_turn.as_deref(),
        Some("base\n\ndeveloper\n\nuser\n\nrunner")
    );
    assert_eq!(resumed_turn.as_deref(), Some("developer\n\nuser\n\nrunner"));
}

#[test]
fn compose_message_instructions_ignores_empty_values() {
    let instructions =
        compose_external_cli_message_instructions(true, Some(" \n "), None, Some(""), Some("\t"));

    assert_eq!(instructions, None);
}

#[test]
fn compose_message_instructions_puts_trusted_channel_context_last() {
    let instructions = compose_external_cli_message_instructions_with_channel_context(
        true,
        Some("base"),
        Some("developer"),
        Some("user"),
        Some("runner"),
        Some("trusted IM route"),
    );

    assert_eq!(
        instructions.as_deref(),
        Some("base\n\ndeveloper\n\nuser\n\nrunner\n\ntrusted IM route")
    );
}

#[test]
fn prompt_persistence_summary_does_not_store_dynamic_routing_context() {
    let summary = prompt_persistence_summary(
        "Provider ID: secret-provider\nExact destination: chat_id=secret-chat",
        0,
        0,
    );

    assert!(summary.contains("\"bytes\""));
    assert!(!summary.contains("secret-provider"));
    assert!(!summary.contains("secret-chat"));
}

#[test]
fn default_gateway_config_contains_enabled_codex_and_traex_runners() {
    let config = ExternalCliGatewayConfig::default();

    assert_eq!(config.default_runner_id, DEFAULT_CODEX_RUNNER_ID);
    let codex = config
        .runners
        .get(DEFAULT_CODEX_RUNNER_ID)
        .expect("Codex default runner");
    assert!(codex.enabled);
    assert_eq!(codex.adapter, DEFAULT_ADAPTER);
    assert!(!codex.inject_bifrost_tools);
    let traex_runner = config
        .runners
        .get(DEFAULT_TRAEX_RUNNER_ID)
        .expect("Traex default runner");
    assert!(traex_runner.enabled);
    assert_eq!(traex_runner.adapter, TRAEX_ADAPTER);
    assert!(!traex_runner.inject_bifrost_tools);
    let claude_code = config
        .runners
        .get(DEFAULT_CLAUDE_CODE_RUNNER_ID)
        .expect("Claude Code default runner");
    assert!(claude_code.enabled);
    assert_eq!(claude_code.adapter, CLAUDE_CODE_ADAPTER);
    assert!(!claude_code.inject_bifrost_tools);
}

#[test]
fn normalized_gateway_config_disables_retired_bifrost_tool_injection() {
    let normalized = normalized_gateway_config(ExternalCliGatewayConfig {
        version: CONFIG_VERSION,
        default_runner_id: "legacy".to_string(),
        runners: BTreeMap::from([(
            "legacy".to_string(),
            ExternalCliAgentSettings {
                enabled: true,
                adapter: "mock".to_string(),
                inject_bifrost_tools: true,
                ..Default::default()
            },
        )]),
        channels: BTreeMap::new(),
    });

    assert_eq!(normalized.version, CONFIG_VERSION);
    assert!(!normalized.runners["legacy"].inject_bifrost_tools);
}

#[test]
fn normalized_gateway_config_adds_named_defaults_without_overwriting_existing_runners() {
    let mut config = ExternalCliGatewayConfig {
        default_runner_id: "custom".to_string(),
        runners: BTreeMap::from([(
            "custom".to_string(),
            ExternalCliAgentSettings {
                enabled: false,
                adapter: "mock".to_string(),
                ..Default::default()
            },
        )]),
        ..Default::default()
    };
    config.runners.remove(DEFAULT_CODEX_RUNNER_ID);
    config.runners.remove(DEFAULT_TRAEX_RUNNER_ID);
    config.runners.remove(DEFAULT_CLAUDE_CODE_RUNNER_ID);

    let normalized = normalized_gateway_config(config);

    assert_eq!(normalized.default_runner_id, "custom");
    assert_eq!(
        normalized
            .runners
            .get("custom")
            .map(|settings| settings.adapter.as_str()),
        Some("mock")
    );
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_CODEX_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, DEFAULT_ADAPTER))
    );
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_TRAEX_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, TRAEX_ADAPTER))
    );
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_CLAUDE_CODE_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, CLAUDE_CODE_ADAPTER))
    );
}

#[test]
fn normalized_gateway_config_migrates_legacy_claude_code_runner_id() {
    let config = ExternalCliGatewayConfig {
        default_runner_id: "Claude Code".to_string(),
        runners: BTreeMap::from([(
            "Claude Code".to_string(),
            ExternalCliAgentSettings {
                enabled: true,
                adapter: CLAUDE_CODE_ADAPTER.to_string(),
                ..Default::default()
            },
        )]),
        channels: BTreeMap::from([(
            "feishu-main".to_string(),
            ExternalCliChannelSettings {
                runner_id: Some("Claude Code".to_string()),
                ..Default::default()
            },
        )]),
        version: 1,
    };

    let normalized = normalized_gateway_config(config);

    assert!(!normalized.runners.contains_key("Claude Code"));
    assert_eq!(normalized.default_runner_id, DEFAULT_CLAUDE_CODE_RUNNER_ID);
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_CLAUDE_CODE_RUNNER_ID)
            .map(|settings| settings.adapter.as_str()),
        Some(CLAUDE_CODE_ADAPTER)
    );
    assert_eq!(
        normalized
            .channels
            .get("feishu-main")
            .and_then(|channel| channel.runner_id.as_deref()),
        Some(DEFAULT_CLAUDE_CODE_RUNNER_ID)
    );
    assert_eq!(
        canonical_external_cli_runner_id(&normalized, "claude code"),
        DEFAULT_CLAUDE_CODE_RUNNER_ID
    );
}

#[test]
fn normalized_gateway_config_empty_runners_uses_enabled_named_defaults() {
    let normalized = normalized_gateway_config(ExternalCliGatewayConfig {
        default_runner_id: "codex".to_string(),
        runners: BTreeMap::new(),
        channels: BTreeMap::new(),
        version: 0,
    });

    assert_eq!(normalized.default_runner_id, DEFAULT_CODEX_RUNNER_ID);
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_CODEX_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, DEFAULT_ADAPTER))
    );
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_TRAEX_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, TRAEX_ADAPTER))
    );
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_CLAUDE_CODE_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, CLAUDE_CODE_ADAPTER))
    );
    assert!(!normalized.runners.contains_key("codex"));
}

#[test]
fn effective_config_resolves_legacy_runner_aliases_to_named_defaults() {
    let config = ExternalCliGatewayConfig::default();

    let codex = effective_config_for_provider_and_runner(&config, None, Some("codex"));
    assert_eq!(codex.runner_id, DEFAULT_CODEX_RUNNER_ID);
    assert_eq!(codex.settings.adapter, DEFAULT_ADAPTER);
    assert!(codex.settings.enabled);

    let traex = effective_config_for_provider_and_runner(&config, None, Some("traex"));
    assert_eq!(traex.runner_id, DEFAULT_TRAEX_RUNNER_ID);
    assert_eq!(traex.settings.adapter, TRAEX_ADAPTER);
    assert!(traex.settings.enabled);

    let legacy_alias = ["Tree", "X"].concat();
    let legacy_traex = effective_config_for_provider_and_runner(&config, None, Some(&legacy_alias));
    assert_eq!(legacy_traex.runner_id, DEFAULT_TRAEX_RUNNER_ID);
    assert_eq!(legacy_traex.settings.adapter, TRAEX_ADAPTER);
    assert!(legacy_traex.settings.enabled);

    let claude_code = effective_config_for_provider_and_runner(&config, None, Some("claude-code"));
    assert_eq!(claude_code.runner_id, DEFAULT_CLAUDE_CODE_RUNNER_ID);
    assert_eq!(claude_code.settings.adapter, CLAUDE_CODE_ADAPTER);
    assert!(claude_code.settings.enabled);
}

#[test]
fn normalized_gateway_config_migrates_legacy_traex_runner_id() {
    let legacy_alias = ["Tree", "X"].concat();
    let normalized = normalized_gateway_config(ExternalCliGatewayConfig {
        default_runner_id: legacy_alias.clone(),
        runners: BTreeMap::from([(
            legacy_alias.clone(),
            ExternalCliAgentSettings {
                enabled: true,
                adapter: TRAEX_ADAPTER.to_string(),
                ..Default::default()
            },
        )]),
        channels: BTreeMap::from([(
            "feishu-main".to_string(),
            ExternalCliChannelSettings {
                runner_id: Some(legacy_alias.clone()),
                ..Default::default()
            },
        )]),
        version: 1,
    });

    assert_eq!(normalized.default_runner_id, DEFAULT_TRAEX_RUNNER_ID);
    assert!(normalized.runners.contains_key(DEFAULT_TRAEX_RUNNER_ID));
    assert!(!normalized.runners.contains_key(&legacy_alias));
    assert_eq!(
        normalized
            .channels
            .get("feishu-main")
            .and_then(|channel| channel.runner_id.as_deref()),
        Some(DEFAULT_TRAEX_RUNNER_ID)
    );
}

#[test]
fn config_store_new_persists_missing_default_runners_on_startup() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir
        .path()
        .join("admin")
        .join("im_gateway_external_cli_agent.json");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(
        &config_path,
        r#"{"version":1,"defaultRunnerId":"legacy","runners":{"legacy":{"enabled":true,"adapter":"mock","adapterConfig":{},"injectBifrostTools":true,"skillPaths":[],"deliveryMode":"final_reply"}},"channels":{}}"#,
    )
    .unwrap();

    let store = ExternalCliConfigStore::new(temp_dir.path());
    let loaded = store.load();
    let persisted: ExternalCliGatewayConfig =
        serde_json::from_str(&std::fs::read_to_string(config_path).unwrap()).unwrap();

    for config in [loaded, persisted] {
        assert!(config.runners.contains_key("legacy"));
        assert!(config.runners.contains_key(DEFAULT_CODEX_RUNNER_ID));
        assert!(config.runners.contains_key(DEFAULT_TRAEX_RUNNER_ID));
        assert!(config.runners.contains_key(DEFAULT_CLAUDE_CODE_RUNNER_ID));
    }
}

#[test]
fn codex_request_metadata_includes_configured_or_default_model_label() {
    let _env_lock = external_cli_env_guard();
    let codex_home = tempfile::tempdir().unwrap();
    let trae_home = tempfile::tempdir().unwrap();
    let _codex_home = EnvGuard::set("CODEX_HOME", codex_home.path());
    let _trae_home = EnvGuard::set("TRAE_HOME", trae_home.path());
    let configured_request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: None,
        runner_id: Some("codex".to_string()),
        session_key: None,
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "codex".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            model: Some("gpt-test".to_string()),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let mut configured_metadata = std::collections::BTreeMap::new();

    append_external_cli_request_metadata(&configured_request, &mut configured_metadata);

    assert_eq!(
        configured_metadata.get("model").map(String::as_str),
        Some("gpt-test")
    );
    assert_eq!(
        configured_metadata.get("modelSource").map(String::as_str),
        Some("runner config")
    );
    assert_eq!(
        configured_metadata.get("modelLabel").map(String::as_str),
        Some("gpt-test")
    );

    let default_request = ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: None,
        runner_id: Some("codex".to_string()),
        session_key: None,
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "codex".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let mut default_metadata = std::collections::BTreeMap::new();

    append_external_cli_request_metadata(&default_request, &mut default_metadata);

    assert_eq!(default_metadata.get("model"), None);
    assert_eq!(
        default_metadata.get("modelSource").map(String::as_str),
        Some("codex default")
    );
    assert_eq!(
        default_metadata.get("modelLabel").map(String::as_str),
        Some("Codex default model (not explicitly configured)")
    );

    let trae_request = ExternalCliRunRequest {
        adapter: TRAEX_ADAPTER.to_string(),
        runner_id: Some("traex".to_string()),
        ..default_request.clone()
    };
    let mut trae_metadata = std::collections::BTreeMap::new();

    append_external_cli_request_metadata(&trae_request, &mut trae_metadata);

    assert_eq!(trae_metadata.get("model"), None);
    assert_eq!(
        trae_metadata.get("modelSource").map(String::as_str),
        Some("trae default")
    );
    assert_eq!(
        trae_metadata.get("modelLabel").map(String::as_str),
        Some("Trae default model (not explicitly configured)")
    );

    let claude_code_request = ExternalCliRunRequest {
        adapter: CLAUDE_CODE_ADAPTER.to_string(),
        runner_id: Some(DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string()),
        ..default_request
    };
    let mut claude_code_metadata = std::collections::BTreeMap::new();

    append_external_cli_request_metadata(&claude_code_request, &mut claude_code_metadata);

    assert_eq!(claude_code_metadata.get("model"), None);
    assert_eq!(
        claude_code_metadata.get("modelSource").map(String::as_str),
        Some("claude code default")
    );
    assert_eq!(
        claude_code_metadata.get("modelLabel").map(String::as_str),
        Some("Claude Code default model (not explicitly configured)")
    );
}

#[test]
fn codex_and_traex_model_config_resolves_user_defaults_and_overrides() {
    let _env_lock = external_cli_env_guard();
    let codex_home = tempfile::tempdir().unwrap();
    let trae_home = tempfile::tempdir().unwrap();
    std::fs::write(
        codex_home.path().join("config.toml"),
        r#"
model = "gpt-codex-default"
model_reasoning_effort = "high"
model_reasoning_summary = "auto"
"#,
    )
    .unwrap();
    std::fs::write(
        codex_home.path().join("work.config.toml"),
        r#"
model = "gpt-codex-profile"
model_reasoning_effort = "medium"
"#,
    )
    .unwrap();
    std::fs::write(
        trae_home.path().join("traecli.toml"),
        r#"
model = "GPT-Trae"
model_provider = "trae"
"#,
    )
    .unwrap();
    let _codex_home = EnvGuard::set("CODEX_HOME", codex_home.path());
    let _trae_home = EnvGuard::set("TRAE_HOME", trae_home.path());

    let codex = resolve_external_cli_model_config(
        DEFAULT_ADAPTER,
        &ExternalCliAdapterConfig {
            profile: Some("work".to_string()),
            ..Default::default()
        },
    );
    assert_eq!(codex.model.as_deref(), Some("gpt-codex-profile"));
    assert_eq!(codex.reasoning_effort.as_deref(), Some("medium"));
    assert_eq!(codex.reasoning_summary.as_deref(), Some("auto"));
    assert_eq!(codex.model_source.as_deref(), Some("codex config"));

    let trae =
        resolve_external_cli_model_config(TRAEX_ADAPTER, &ExternalCliAdapterConfig::default());
    assert_eq!(trae.model.as_deref(), Some("GPT-Trae"));
    assert_eq!(trae.model_provider.as_deref(), Some("trae"));

    let overridden = resolve_external_cli_model_config(
        DEFAULT_ADAPTER,
        &ExternalCliAdapterConfig {
            model: Some("gpt-runner".to_string()),
            reasoning_effort: Some("low".to_string()),
            config_overrides: vec!["model_reasoning_summary=\"detailed\"".to_string()],
            ..Default::default()
        },
    );
    assert_eq!(overridden.model.as_deref(), Some("gpt-runner"));
    assert_eq!(overridden.reasoning_effort.as_deref(), Some("low"));
    assert_eq!(overridden.reasoning_summary.as_deref(), Some("detailed"));
    assert_eq!(overridden.model_source.as_deref(), Some("runner config"));
    assert_eq!(
        overridden.reasoning_source.as_deref(),
        Some("runner config")
    );
}

#[test]
fn claude_code_model_config_ignores_settings_model_but_resolves_effort() {
    let _env_lock = external_cli_env_guard();
    let home = tempfile::tempdir().unwrap();
    let claude_home = home.path().join(".claude");
    std::fs::create_dir_all(&claude_home).unwrap();
    std::fs::write(
        claude_home.join("settings.json"),
        r#"{
          "model": "sonnet",
          "effortLevel": "low",
          "env": {
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": "claude-haiku-custom",
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-opus-4-7",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-opus-custom",
            "CLAUDE_CODE_EFFORT_LEVEL": "medium"
          }
        }"#,
    )
    .unwrap();
    let _home = EnvGuard::set("HOME", home.path());
    let _claude_config_dir = EnvGuard::unset("CLAUDE_CONFIG_DIR");
    let _claude_home = EnvGuard::unset("CLAUDE_HOME");
    let _anthropic_model = EnvGuard::unset("ANTHROPIC_MODEL");
    let _default_sonnet = EnvGuard::unset("ANTHROPIC_DEFAULT_SONNET_MODEL");
    let _default_opus = EnvGuard::unset("ANTHROPIC_DEFAULT_OPUS_MODEL");
    let _default_haiku = EnvGuard::unset("ANTHROPIC_DEFAULT_HAIKU_MODEL");
    let _claude_effort = EnvGuard::unset("CLAUDE_CODE_EFFORT_LEVEL");
    let _claude_effort_short = EnvGuard::unset("CLAUDE_EFFORT");

    let claude = resolve_external_cli_model_config(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig::default(),
    );
    assert_eq!(claude.model, None);
    assert_eq!(claude.model_provider, None);
    assert_eq!(claude.model_source, None);
    assert_eq!(claude.reasoning_effort.as_deref(), Some("medium"));
    assert_eq!(claude.reasoning_source.as_deref(), Some("claude settings"));

    let runner_model = resolve_external_cli_model_config(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig {
            env: BTreeMap::from([(
                "ANTHROPIC_MODEL".to_string(),
                "custom-direct-model".to_string(),
            )]),
            ..Default::default()
        },
    );
    assert_eq!(runner_model.model, None);
    assert_eq!(runner_model.reasoning_effort.as_deref(), Some("medium"));

    let runner_effort = resolve_external_cli_model_config(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig {
            env: BTreeMap::from([("CLAUDE_CODE_EFFORT_LEVEL".to_string(), "high".to_string())]),
            ..Default::default()
        },
    );
    assert_eq!(runner_effort.model, None);
    assert_eq!(runner_effort.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(
        runner_effort.reasoning_source.as_deref(),
        Some("runner config")
    );

    let status = resolve_external_cli_status_model_config(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig::default(),
    );
    assert_eq!(status.model.as_deref(), Some("claude-opus-4-7"));
    assert_eq!(status.model_provider.as_deref(), Some("sonnet"));
    assert_eq!(status.model_source.as_deref(), Some("claude settings"));
    assert_eq!(status.reasoning_effort.as_deref(), Some("medium"));
}

#[test]
fn claude_code_status_model_config_reads_plain_settings_model_without_catalog_coupling() {
    let _env_lock = external_cli_env_guard();
    let home = tempfile::tempdir().unwrap();
    let claude_home = home.path().join(".claude");
    std::fs::create_dir_all(&claude_home).unwrap();
    std::fs::write(
        claude_home.join("settings.json"),
        r#"{
          "model": "opus",
          "effortLevel": "low"
        }"#,
    )
    .unwrap();
    let _home = EnvGuard::set("HOME", home.path());
    let _claude_config_dir = EnvGuard::unset("CLAUDE_CONFIG_DIR");
    let _claude_home = EnvGuard::unset("CLAUDE_HOME");
    let _anthropic_model = EnvGuard::unset("ANTHROPIC_MODEL");
    let _default_sonnet = EnvGuard::unset("ANTHROPIC_DEFAULT_SONNET_MODEL");
    let _default_opus = EnvGuard::unset("ANTHROPIC_DEFAULT_OPUS_MODEL");
    let _default_haiku = EnvGuard::unset("ANTHROPIC_DEFAULT_HAIKU_MODEL");
    let _claude_effort = EnvGuard::unset("CLAUDE_CODE_EFFORT_LEVEL");
    let _claude_effort_short = EnvGuard::unset("CLAUDE_EFFORT");

    let runtime = resolve_external_cli_model_config(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig::default(),
    );
    assert_eq!(runtime.model, None);
    assert_eq!(runtime.reasoning_effort.as_deref(), Some("low"));

    let status = resolve_external_cli_status_model_config(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig::default(),
    );
    assert_eq!(status.model.as_deref(), Some("opus"));
    assert_eq!(status.model_provider, None);
    assert_eq!(status.model_source.as_deref(), Some("claude settings"));
    assert_eq!(status.reasoning_effort.as_deref(), Some("low"));
}

#[test]
fn codex_like_metadata_includes_turn_usage_tokens() {
    let events = parse_progress_events(
        r#"{"type":"thread.started","thread_id":"thread-usage"}
{"type":"turn.completed","usage":{"input_tokens":59589,"cached_input_tokens":6912,"output_tokens":221,"reasoning_output_tokens":156}}"#,
    );
    let mut metadata = std::collections::BTreeMap::new();

    append_external_cli_metadata(TRAEX_ADAPTER, &events, &mut metadata);

    assert_eq!(
        metadata.get("threadId").map(String::as_str),
        Some("thread-usage")
    );
    assert_eq!(
        metadata.get("usageInputTokens").map(String::as_str),
        Some("59589")
    );
    assert_eq!(
        metadata.get("usageCachedInputTokens").map(String::as_str),
        Some("6912")
    );
    assert_eq!(
        metadata.get("usageOutputTokens").map(String::as_str),
        Some("221")
    );
    assert_eq!(
        metadata
            .get("usageReasoningOutputTokens")
            .map(String::as_str),
        Some("156")
    );
    assert_eq!(
        metadata.get("usageTotalTokens").map(String::as_str),
        Some("59810")
    );
}

#[test]
fn codex_progress_metadata_merges_thread_total_and_weekly_window() {
    let usage_event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::Status,
        content: "token usage updated".to_string(),
        title: Some("token_usage".to_string()),
        raw: serde_json::json!({
            "usage": {
                "input_tokens": 1200,
                "cached_input_tokens": 300,
                "output_tokens": 80,
                "reasoning_output_tokens": 20,
                "total_tokens": 1280
            }
        }),
    };
    let limits_event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::Status,
        content: "usage updated".to_string(),
        title: Some("rate_limits".to_string()),
        raw: serde_json::json!({
            "params": {
                "rateLimits": {
                    "limitId": "codex",
                    "primary": {
                        "usedPercent": 63,
                        "windowDurationMins": 10080,
                        "resetsAt": 1784490086
                    },
                    "secondary": {
                        "usedPercent": 5,
                        "windowDurationMins": 300,
                        "resetsAt": 1784000000
                    }
                }
            }
        }),
    };
    let mut metadata = std::collections::BTreeMap::new();

    assert!(merge_external_cli_progress_metadata(
        DEFAULT_ADAPTER,
        &usage_event,
        &mut metadata
    ));
    assert!(merge_external_cli_progress_metadata(
        DEFAULT_ADAPTER,
        &limits_event,
        &mut metadata
    ));

    assert_eq!(
        metadata.get("usageTotalTokens").map(String::as_str),
        Some("1280")
    );
    assert_eq!(
        metadata.get("codexWeeklyUsedPercent").map(String::as_str),
        Some("63")
    );
    assert_eq!(
        metadata.get("codexWeeklyWindowMinutes").map(String::as_str),
        Some("10080")
    );
    assert_eq!(
        metadata.get("codexWeeklyResetsAt").map(String::as_str),
        Some("1784490086")
    );
}

#[test]
fn codex_progress_metadata_ignores_short_windows_and_non_codex_adapters() {
    let event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::Status,
        content: "usage updated".to_string(),
        title: Some("rate_limits".to_string()),
        raw: serde_json::json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 20,
                    "windowDurationMins": 300,
                    "resetsAt": 1784000000
                }
            }
        }),
    };
    let mut metadata = std::collections::BTreeMap::new();

    assert!(!merge_external_cli_progress_metadata(
        DEFAULT_ADAPTER,
        &event,
        &mut metadata
    ));
    assert!(!merge_external_cli_progress_metadata(
        CLAUDE_CODE_ADAPTER,
        &event,
        &mut metadata
    ));
    assert!(metadata.is_empty());
}

#[test]
fn codex_like_progress_metadata_captures_target_runner_session_id_immediately() {
    for (adapter, raw, expected) in [
        (
            DEFAULT_ADAPTER,
            serde_json::json!({"type":"thread.started","thread_id":"codex-thread-live"}),
            "codex-thread-live",
        ),
        (
            TRAEX_ADAPTER,
            serde_json::json!({"type":"thread.started","threadId":"traex-thread-live"}),
            "traex-thread-live",
        ),
        (
            CLAUDE_CODE_ADAPTER,
            serde_json::json!({"type":"system","subtype":"init","session_id":"claude-session-live"}),
            "claude-session-live",
        ),
    ] {
        let event = ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::RunStarted,
            content: expected.to_string(),
            title: Some("runner session".to_string()),
            raw,
        };
        let mut metadata = std::collections::BTreeMap::new();

        assert!(merge_external_cli_progress_metadata(
            adapter,
            &event,
            &mut metadata
        ));
        assert_eq!(metadata.get("threadId").map(String::as_str), Some(expected));
    }
}

#[test]
fn codex_and_traex_metadata_include_runner_observability() {
    for adapter in [DEFAULT_ADAPTER, TRAEX_ADAPTER] {
        let request = ExternalCliRunRequest {
            images: Vec::new(),
            files: Vec::new(),
            message: "inspect image".to_string(),
            operation: default_operation(),
            params: serde_json::json!({"threadId": "thread-existing"}),
            provider_id: Some("web".to_string()),
            runner_id: Some(adapter.to_string()),
            session_key: Some("session-observe".to_string()),
            runtime: DEFAULT_RUNTIME.to_string(),
            adapter: adapter.to_string(),
            work_dir: Some(std::path::PathBuf::from("/tmp/work")),
            instructions: None,
            adapter_config: ExternalCliAdapterConfig {
                approval_policy: Some("never".to_string()),
                sandbox: Some("danger-full-access".to_string()),
                permission_mode: Some("bypassPermissions".to_string()),
                danger_full_access: Some(true),
                add_dirs: vec!["/tmp/extra".to_string()],
                enable_features: vec!["network".to_string()],
                timeout_secs: Some(30),
                ..Default::default()
            },
            allow_work_dirs: Vec::new(),
            inject_bifrost_tools: true,
            skill_paths: Vec::new(),
        };
        let spec = CommandSpec {
            executable: adapter.to_string(),
            args: vec!["exec".to_string(), "--json".to_string()],
            env: std::collections::BTreeMap::new(),
            work_dir: request.work_dir.clone(),
            timeout_secs: Some(30),
        };
        let events = vec![
            ExternalCliProgressEvent {
                event_type: ExternalCliProgressEventType::Status,
                content: "retrying capacity error".to_string(),
                title: Some("Codex capacity retry".to_string()),
                raw: serde_json::json!({
                    "type": "capacity_retry",
                    "retryAttempt": 1,
                    "maxRetries": 3,
                    "delayMs": 1000
                }),
            },
            ExternalCliProgressEvent {
                event_type: ExternalCliProgressEventType::ToolFinished,
                content: "tool output".to_string(),
                title: Some("Shell".to_string()),
                raw: serde_json::json!({
                    "type": "item.completed",
                    "observedAtMs": 1120,
                    "durationMs": 120,
                    "item": {
                        "id": "tool-1",
                        "type": "command_execution",
                        "command": "pwd",
                        "exit_code": 0,
                        "status": "completed"
                    }
                }),
            },
            ExternalCliProgressEvent {
                event_type: ExternalCliProgressEventType::AssistantFinal,
                content: "done".to_string(),
                title: None,
                raw: serde_json::json!({"type": "assistant_final", "observedAtMs": 1150}),
            },
        ];
        let saved_images = vec![ExternalCliSavedImageAttachment {
            path: "/tmp/session/run/images/image-1.png".to_string(),
            mime_type: "image/png".to_string(),
            size_bytes: 42,
            name: Some("image.png".to_string()),
        }];
        let saved_files = Vec::new();
        let mut metadata = std::collections::BTreeMap::new();

        append_external_cli_observability_metadata(
            ExternalCliObservabilityInput {
                request: &request,
                spec: &spec,
                prompt: "## Attached Images\n- /tmp/session/run/images/image-1.png\n",
                saved_images: &saved_images,
                saved_files: &saved_files,
                stdout: b"{\"type\":\"assistant_final\"}\n",
                stderr: b"warning\n",
                events: &events,
                timings: ExternalCliObservabilityTimings {
                    started_at: 1000,
                    command_started_at: Some(1010),
                    command_finished_at: Some(1200),
                    finished_at: 1250,
                },
                cli_version: Some("runner 1.2.3"),
            },
            &mut metadata,
        );

        assert_eq!(
            metadata.get("runner.adapter").map(String::as_str),
            Some(adapter)
        );
        assert_eq!(
            metadata.get("cli.version").map(String::as_str),
            Some("runner 1.2.3")
        );
        assert_eq!(
            metadata.get("prompt.attachmentPathCount"),
            Some(&"1".to_string())
        );
        assert_eq!(
            metadata.get("attachments.totalBytes"),
            Some(&"42".to_string())
        );
        assert_eq!(metadata.get("io.stdoutLines"), Some(&"1".to_string()));
        assert_eq!(
            metadata.get("timing.commandDurationMs"),
            Some(&"190".to_string())
        );
        assert_eq!(
            metadata.get("timing.firstEventLatencyMs"),
            Some(&"120".to_string())
        );
        assert_eq!(metadata.get("tools.count"), Some(&"1".to_string()));
        assert_eq!(
            metadata.get("runner.capacityRetryCount"),
            Some(&"1".to_string())
        );
        assert_eq!(
            metadata.get("tools.totalDurationMs"),
            Some(&"120".to_string())
        );
        assert_eq!(
            metadata.get("resume.requested").map(String::as_str),
            Some("true")
        );
    }
}

#[test]
fn progress_event_observation_adds_tool_duration() {
    let mut starts = std::collections::HashMap::new();
    let mut started = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolStarted,
        content: "pwd".to_string(),
        title: None,
        raw: serde_json::json!({"type": "item.started", "item": {"id": "tool-1"}}),
    };
    let mut finished = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolFinished,
        content: "/tmp".to_string(),
        title: None,
        raw: serde_json::json!({"type": "item.completed", "item": {"id": "tool-1"}}),
    };

    enrich_progress_event_observation(&mut started, 2000, &mut starts);
    enrich_progress_event_observation(&mut finished, 2125, &mut starts);

    assert_eq!(
        started
            .raw
            .get("observedAtMs")
            .and_then(serde_json::Value::as_u64),
        Some(2000)
    );
    assert_eq!(
        finished
            .raw
            .get("observedAtMs")
            .and_then(serde_json::Value::as_u64),
        Some(2125)
    );
    assert_eq!(
        finished
            .raw
            .get("durationMs")
            .and_then(serde_json::Value::as_u64),
        Some(125)
    );
}

#[test]
fn progress_event_observation_tracks_and_freezes_subagent_duration() {
    let mut starts = std::collections::HashMap::new();
    let mut running = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::SubAgentUpdated,
        content: String::new(),
        title: None,
        raw: serde_json::json!({
            "subagent": {"id": "agent-1", "status": "running"}
        }),
    };
    let mut completed = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::SubAgentUpdated,
        content: String::new(),
        title: None,
        raw: serde_json::json!({
            "subagent": {"id": "agent-1", "status": "completed"}
        }),
    };

    enrich_progress_event_observation(&mut running, 2_000, &mut starts);
    assert_eq!(running.raw["observedAtMs"], 2_000);
    assert_eq!(starts.get("agent-1"), Some(&2_000));

    enrich_progress_event_observation(&mut completed, 2_450, &mut starts);
    assert_eq!(completed.raw["subagent"]["startedAtMs"], 2_000);
    assert_eq!(completed.raw["subagent"]["durationMs"], 450);
    assert!(starts.is_empty());
}

#[test]
fn legacy_subagent_status_fallbacks_remain_compatible_for_history_replay() {
    assert_eq!(
        normalize_codex_subagent_status(Some("pendingInit"), None, false),
        "pending"
    );
    assert_eq!(
        normalize_codex_subagent_status(None, Some("failed"), false),
        "failed"
    );
    assert_eq!(
        normalize_codex_subagent_status(None, Some("completed"), false),
        "completed"
    );
    assert_eq!(
        normalize_codex_subagent_status(None, Some("in_progress"), false),
        "running"
    );
    assert_eq!(
        normalize_codex_subagent_status(None, None, true),
        "completed"
    );
    assert_eq!(
        normalize_codex_subagent_status(None, None, false),
        "running"
    );

    let unknown = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::SubAgentUpdated,
        content: String::new(),
        title: None,
        raw: serde_json::json!({"subagent": {"id": "unknown-1", "status": "new-state"}}),
    };
    assert_eq!(
        external_progress_subagent(&unknown).unwrap().status,
        bifrost_agent::SubAgentStatus::Unknown
    );
}

#[test]
fn claude_code_task_error_without_interrupt_is_a_failed_plain_tool() {
    let events = parse_progress_events(
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"task-failed","name":"Task","input":{"prompt":"Review auth"}}]}}
{"type":"user","message":{"content":[{"tool_use_id":"task-failed","type":"tool_result","content":"Permission denied","is_error":true}]},"tool_use_result":{"interrupted":false}}"#,
    );
    assert_eq!(
        events[1].event_type,
        ExternalCliProgressEventType::ToolFinished
    );
    assert_eq!(events[1].title.as_deref(), Some("Task"));
    assert_eq!(events[1].content, "Permission denied");
    assert_eq!(events[1].raw["success"], false);
}

#[test]
fn codex_cli_parser_maps_reasoning_summary_to_assistant_delta() {
    let events = parse_progress_events(
        r#"{"type":"item.completed","item":{"id":"reasoning_0","type":"reasoning_summary","summary":"I checked the workspace and will run the focused tests."}}"#,
    );

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::AssistantDelta
    );
    assert_eq!(
        events[0].content,
        "I checked the workspace and will run the focused tests."
    );
}

#[test]
fn traex_model_slash_command_parser_handles_list_show_set_and_clear() {
    assert_eq!(
        parse_external_cli_model_slash_command("/models"),
        Some(Ok(ExternalCliModelSlashCommand::List))
    );
    assert!(matches!(
        parse_external_cli_model_slash_command("/models extra"),
        Some(Err(_))
    ));
    assert_eq!(
        parse_external_cli_model_slash_command(" /model "),
        Some(Ok(ExternalCliModelSlashCommand::Show))
    );
    assert_eq!(
        parse_external_cli_model_slash_command("/model gpt-5.5"),
        Some(Ok(ExternalCliModelSlashCommand::Set("gpt-5.5".to_string())))
    );
    assert_eq!(
        parse_external_cli_model_slash_command("/model clear"),
        Some(Ok(ExternalCliModelSlashCommand::Clear))
    );
    assert!(matches!(
        parse_external_cli_model_slash_command("/model bad model"),
        Some(Err(_))
    ));
    assert_eq!(parse_external_cli_model_slash_command("/modelish"), None);
}

#[test]
fn external_cli_effort_slash_command_parser_handles_list_show_set_and_clear() {
    assert_eq!(
        parse_external_cli_effort_slash_command("/efforts"),
        Some(Ok(ExternalCliEffortSlashCommand::List))
    );
    assert!(matches!(
        parse_external_cli_effort_slash_command("/efforts extra"),
        Some(Err(_))
    ));
    assert_eq!(
        parse_external_cli_effort_slash_command(" /effort "),
        Some(Ok(ExternalCliEffortSlashCommand::Show))
    );
    assert_eq!(
        parse_external_cli_effort_slash_command("/effort xhigh"),
        Some(Ok(ExternalCliEffortSlashCommand::Set("xhigh".to_string())))
    );
    assert_eq!(
        parse_external_cli_effort_slash_command("/effort clear"),
        Some(Ok(ExternalCliEffortSlashCommand::Clear))
    );
    assert_eq!(
        parse_external_cli_effort_slash_command("/effort auto"),
        Some(Ok(ExternalCliEffortSlashCommand::Clear))
    );
    assert!(matches!(
        parse_external_cli_effort_slash_command("/effort bad value"),
        Some(Err(_))
    ));
    assert_eq!(parse_external_cli_effort_slash_command("/effortish"), None);
}

#[test]
fn codex_fast_slash_command_parser_handles_toggle_on_off_status_and_errors() {
    assert_eq!(
        parse_external_cli_fast_slash_command(" /fast "),
        Some(Ok(ExternalCliFastSlashCommand::Toggle))
    );
    assert_eq!(
        parse_external_cli_fast_slash_command("/FAST ON"),
        Some(Ok(ExternalCliFastSlashCommand::On))
    );
    assert_eq!(
        parse_external_cli_fast_slash_command("/fast off"),
        Some(Ok(ExternalCliFastSlashCommand::Off))
    );
    assert_eq!(
        parse_external_cli_fast_slash_command("/fast status"),
        Some(Ok(ExternalCliFastSlashCommand::Status))
    );
    assert!(matches!(
        parse_external_cli_fast_slash_command("/fast maybe"),
        Some(Err(_))
    ));
    assert_eq!(parse_external_cli_fast_slash_command("/fastish"), None);
}

#[test]
fn external_cli_model_catalog_parser_filters_raw_catalog_to_safe_public_fields() {
    let models = parse_external_cli_model_catalog(
        TRAEX_ADAPTER,
        r#"{
          "models": [
            {
              "slug": "hidden-model",
              "visibility": "hidden",
              "base_instructions": "do not leak"
            },
            {
              "slug": "Doubao-Seed-2.1-Pro",
              "description": "flagship",
              "default_reasoning_level": "high",
              "supported_reasoning_levels": [{"effort": "low", "description": "fast"}],
              "visibility": "list",
              "supported_in_api": true,
              "model_load": 115,
              "priority": 2,
              "additional_speed_tiers": ["fast"],
              "service_tiers": [{"id": "default", "name": "Default", "description": "standard"}],
              "base_instructions": "do not leak"
            },
            {
              "slug": "DeepSeek-V4-Flash",
              "visibility": "list",
              "priority": 1
            }
          ]
        }"#,
    )
    .expect("parse catalog");

    assert_eq!(models.len(), 2);
    assert_eq!(models[0].slug, "DeepSeek-V4-Flash");
    assert_eq!(models[1].slug, "Doubao-Seed-2.1-Pro");
    assert_eq!(models[1].default_reasoning_level.as_deref(), Some("high"));
    assert_eq!(models[1].additional_speed_tiers, vec!["fast"]);
    assert_eq!(models[1].model_load.as_deref(), Some("115%"));
    let serialized = serde_json::to_string(&models).expect("serialize sanitized catalog");
    assert!(!serialized.contains("base_instructions"));
    assert!(!serialized.contains("do not leak"));
    let rendered = format_external_cli_model_catalog(TRAEX_ADAPTER, &models);
    assert!(rendered.contains("Model load: 115%"));
}

#[test]
fn external_cli_model_catalog_parser_accepts_codex_catalog() {
    let models = parse_external_cli_model_catalog(
        DEFAULT_ADAPTER,
        r#"{
          "models": [
            {
              "slug": "gpt-5.5",
              "description": "Frontier model",
              "default_reasoning_level": "medium",
              "visibility": "list",
              "priority": 0,
              "additional_speed_tiers": ["fast"],
              "base_instructions": "do not leak"
            }
          ]
        }"#,
    )
    .expect("parse codex catalog");

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].slug, "gpt-5.5");
    assert_eq!(models[0].default_reasoning_level.as_deref(), Some("medium"));
    let serialized = serde_json::to_string(&models).expect("serialize sanitized catalog");
    assert!(!serialized.contains("base_instructions"));
    assert!(!serialized.contains("do not leak"));
}

#[tokio::test]
async fn claude_code_model_slash_uses_builtin_catalog_and_accepts_full_model_slug() {
    assert!(supports_external_cli_model_slash(CLAUDE_CODE_ADAPTER));
    assert_eq!(
        external_cli_model_adapter_label(CLAUDE_CODE_ADAPTER),
        "Claude Code"
    );

    let models = load_external_cli_model_catalog(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig::default(),
        None,
    )
    .await
    .expect("load claude code catalog");

    assert!(models.iter().any(|model| model.slug == "sonnet"));
    assert!(models.iter().any(|model| model.slug == "opus"));
    assert!(models.iter().any(|model| model.slug == "haiku"));
    assert!(models.iter().any(|model| model.slug == "fable"));
    let rendered = format_external_cli_model_catalog(CLAUDE_CODE_ADAPTER, &models);
    assert!(rendered.contains("Sonnet 4.6"));
    assert!(rendered.contains("Opus 4.8"));
    assert!(rendered.contains("Haiku 4.5"));
    assert_eq!(
        validate_external_cli_model_selection(CLAUDE_CODE_ADAPTER, "sonnet", &models)
            .expect("known alias"),
        "sonnet"
    );
    assert_eq!(
        validate_external_cli_model_selection(
            CLAUDE_CODE_ADAPTER,
            "claude-opus-4-5-20251101",
            &models,
        )
        .expect("full model name"),
        "claude-opus-4-5-20251101"
    );
    assert!(
        validate_external_cli_model_selection(CLAUDE_CODE_ADAPTER, "bad model", &models,).is_err()
    );
}

#[test]
fn external_cli_effort_validation_uses_runner_specific_options() {
    assert_eq!(
        validate_external_cli_effort_selection(CLAUDE_CODE_ADAPTER, "xhigh").unwrap(),
        "xhigh"
    );
    assert_eq!(
        validate_external_cli_effort_selection(CLAUDE_CODE_ADAPTER, "MAX").unwrap(),
        "max"
    );
    assert!(validate_external_cli_effort_selection(CLAUDE_CODE_ADAPTER, "minimal").is_err());
    assert_eq!(
        validate_external_cli_effort_selection(DEFAULT_ADAPTER, "minimal").unwrap(),
        "minimal"
    );
    assert!(validate_external_cli_effort_selection(DEFAULT_ADAPTER, "max").is_err());
    assert_eq!(
        validate_external_cli_effort_selection(TRAEX_ADAPTER, "high").unwrap(),
        "high"
    );
}

#[test]
fn external_cli_effort_validation_honors_current_model_supported_levels() {
    let models = vec![ExternalCliModelInfo {
        slug: "thinking-model".to_string(),
        default_reasoning_level: Some("medium".to_string()),
        supported_reasoning_levels: vec![
            ExternalCliReasoningLevelInfo {
                effort: "low".to_string(),
                description: None,
            },
            ExternalCliReasoningLevelInfo {
                effort: "medium".to_string(),
                description: None,
            },
        ],
        ..Default::default()
    }];

    assert_eq!(
        validate_external_cli_effort_selection_for_model(
            TRAEX_ADAPTER,
            "low",
            Some("thinking-model"),
            &models,
        )
        .unwrap(),
        "low"
    );
    assert!(validate_external_cli_effort_selection_for_model(
        TRAEX_ADAPTER,
        "high",
        Some("thinking-model"),
        &models,
    )
    .is_err());
    assert_eq!(
        validate_external_cli_effort_selection_for_model(
            TRAEX_ADAPTER,
            "high",
            Some("unknown-model"),
            &models,
        )
        .unwrap(),
        "high"
    );
    let rendered = format_external_cli_effort_catalog_for_model(
        TRAEX_ADAPTER,
        Some("thinking-model"),
        &models,
    );
    assert!(rendered.contains("当前模型 `thinking-model`"));
    assert!(rendered.contains("`low`"));
    assert!(!rendered.contains("`high`"));
}

#[test]
fn external_cli_command_environment_augments_path_unless_explicitly_overridden() {
    let _guard = external_cli_env_guard();
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let system_path = PathBuf::from(
        std::env::join_paths([
            temp_dir.path().join("system-bin"),
            temp_dir.path().join("fallback-bin"),
        ])
        .expect("system path"),
    );
    let _path_guard = EnvGuard::set("PATH", &system_path);

    let mut command = Command::new("traex");
    apply_command_environment(&mut command, &BTreeMap::new());
    let path = command
        .as_std()
        .get_envs()
        .find_map(|(key, value)| (key == OsStr::new("PATH")).then_some(value).flatten())
        .expect("augmented PATH");
    let expected_path = bifrost_core::inherited_executable_path().expect("expected PATH");
    assert_eq!(path, expected_path);

    let configured_path = "/custom/traex/bin";
    let mut env = BTreeMap::new();
    env.insert("PATH".to_string(), configured_path.to_string());
    let mut command = Command::new("traex");
    apply_command_environment(&mut command, &env);
    let path = command
        .as_std()
        .get_envs()
        .find_map(|(key, value)| (key == OsStr::new("PATH")).then_some(value).flatten())
        .expect("configured PATH");
    assert_eq!(path, OsStr::new(configured_path));
}

#[test]
fn external_cli_explicit_path_detection_matches_platform_semantics() {
    let mut env = BTreeMap::new();
    env.insert("PATH".to_string(), "/custom/bin".to_string());
    assert!(has_explicit_path_environment(&env));

    env.clear();
    env.insert("Path".to_string(), "/custom/bin".to_string());
    assert_eq!(has_explicit_path_environment(&env), cfg!(windows));
}

#[test]
fn external_cli_worker_command_sets_internal_worker_marker() {
    let command = external_cli_worker_process_command(Path::new("bifrost"));
    let command = command.as_std();
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let worker_marker = command
        .get_envs()
        .find_map(|(key, value)| (key == OsStr::new(EXTERNAL_CLI_WORKER_ENV)).then_some(value))
        .flatten();

    assert_eq!(args, ["agent", "external-runner-worker"]);
    assert_eq!(worker_marker, Some(OsStr::new("1")));
}

#[test]
fn ambient_worker_marker_does_not_override_forced_worker_delegation() {
    let _guard = external_cli_env_guard();
    let _worker_marker = EnvGuard::set(EXTERNAL_CLI_WORKER_ENV, Path::new("1"));
    let _force_worker = EnvGuard::set("BIFROST_FORCE_EXTERNAL_CLI_WORKER", Path::new("1"));

    assert!(!should_run_external_cli_in_current_process());
}

fn has_arg_pair(args: &[String], left: &str, right: &str) -> bool {
    args.windows(2)
        .any(|pair| pair[0] == left && pair[1] == right)
}

async fn wait_for_single_run_dir(runs_root: &Path) -> String {
    for _ in 0..100 {
        let mut entries = tokio::fs::read_dir(runs_root).await.unwrap();
        if let Some(entry) = entries.next_entry().await.unwrap() {
            return entry.file_name().to_string_lossy().to_string();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("run dir was not created");
}

async fn wait_for_file_text(path: &Path, expected: &str) {
    // Coverage and workspace test runs can schedule thousands of tests at once,
    // so give the spawned protocol mock enough time to receive its first frame.
    for _ in 0..500 {
        if tokio::fs::read_to_string(path)
            .await
            .is_ok_and(|content| content.contains(expected))
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let content = tokio::fs::read_to_string(path).await.unwrap_or_default();
    panic!(
        "file '{}' did not contain '{expected}'; content={content}",
        path.display()
    );
}

#[test]
fn executor_persistence_keeps_only_bounded_ui_summaries() {
    let mut tail = vec![b'a'; MAX_CAPTURED_STREAM_BYTES];
    append_tail(&mut tail, b"xyz", MAX_CAPTURED_STREAM_BYTES);
    assert_eq!(tail.len(), MAX_CAPTURED_STREAM_BYTES);
    assert!(tail.ends_with(b"xyz"));

    let events = vec![
        ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::AssistantDelta,
            content: "transient".to_string(),
            title: None,
            raw: serde_json::json!({"detail": "not archived"}),
        },
        ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::ToolFinished,
            content: "x".repeat(4096),
            title: Some("read_file".to_string()),
            raw: serde_json::json!({
                "call_id": "call-1",
                "tool_name": "read_file",
                "success": true,
                "result": "x".repeat(4096),
            }),
        },
        ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::AssistantFinal,
            content: "done".to_string(),
            title: None,
            raw: serde_json::json!({}),
        },
    ];
    let persisted = persisted_event_summaries(&events);
    assert_eq!(persisted.len(), 1);
    assert!(persisted[0].content.is_empty());
    assert_eq!(persisted[0].raw["_bifrost_compacted"], true);
    assert_eq!(persisted[0].raw["call_id"], "call-1");
    assert_eq!(persisted[0].raw["tool_name"], "read_file");
    assert_eq!(persisted[0].raw["success"], true);
    assert!(persisted[0].raw.get("result").is_none());
    assert_eq!(events[0].content, "transient");
    assert_eq!(events[1].content.len(), 4096);
    assert_eq!(events[2].content, "done");

    let mut oversized = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolFinished,
        content: "done".to_string(),
        title: Some("shell".to_string()),
        raw: serde_json::json!({
            "item": {
                "id": "nested-call",
                "name": "shell",
                "status": "completed",
                "exit_code": 0,
                "aggregated_output": "x".repeat(4096),
            }
        }),
    };
    oversized = compact_persisted_progress_event(oversized);
    assert!(oversized.content.is_empty());
    assert_eq!(oversized.raw["_bifrost_compacted"], true);
    assert_eq!(oversized.raw["id"], "nested-call");
    assert_eq!(oversized.raw["tool_name"], "shell");
    assert_eq!(oversized.raw["status"], "completed");
    assert_eq!(oversized.raw["exit_code"], 0);
    assert!(oversized.raw.get("aggregated_output").is_none());

    let bounded_raw = compacted_progress_raw(&serde_json::json!({
        "call_id": "x".repeat(MAX_PERSISTED_PROGRESS_RAW_STRING_BYTES + 100),
        "tool_name": {"nested": "must not persist"},
        "status": ["must not persist"],
    }));
    assert_eq!(
        bounded_raw["call_id"].as_str().unwrap().len(),
        MAX_PERSISTED_PROGRESS_RAW_STRING_BYTES
    );
    assert!(bounded_raw.get("tool_name").is_none());
    assert!(bounded_raw.get("status").is_none());

    let nested_only = compacted_progress_raw(&serde_json::json!({
        "item": {
            "id": "nested-id",
            "name": "nested-tool",
            "status": "failed",
            "exit_code": 9
        }
    }));
    assert_eq!(nested_only["id"], "nested-id");
    assert_eq!(nested_only["tool_name"], "nested-tool");
    assert_eq!(nested_only["status"], "failed");
    assert_eq!(nested_only["exit_code"], 9);

    let utf8 = truncate_utf8_bytes("éé", 3);
    assert_eq!(utf8, "é");
}

#[test]
fn append_tail_keeps_only_the_bounded_suffix() {
    let mut tail = b"old".to_vec();
    append_tail(&mut tail, b"0123456789", 4);
    assert_eq!(tail, b"6789");
}

#[test]
fn worker_stderr_tail_and_inactive_lock_cover_boundary_paths() {
    let mut tail = b"old".to_vec();
    append_capped_tail(&mut tail, b"012345", 4);
    assert_eq!(tail, b"2345");
    append_capped_tail(&mut tail, b"67", 4);
    assert_eq!(tail, b"4567");
    assert_eq!(format_external_cli_worker_stderr("  "), None);
    let long = format!("{}é", "x".repeat(MAX_CAPTURED_WORKER_STDERR_BYTES));
    assert!(format_external_cli_worker_stderr(&long)
        .unwrap()
        .ends_with('…'));

    let root = tempfile::tempdir().unwrap();
    let run = root.path().join("run");
    std::fs::create_dir_all(run.join(EXTERNAL_CLI_RUN_LOCK_FILE)).unwrap();
    assert!(lock_inactive_external_cli_run(&run).is_none());
    assert!(lock_inactive_external_cli_run(&root.path().join("missing")).is_none());
}

#[test]
fn app_server_stdout_capture_keeps_only_the_bounded_suffix() {
    let mut bytes = Vec::new();
    super::app_server::record_stdout_line(&mut bytes, &"x".repeat(MAX_CAPTURED_STREAM_BYTES * 2));

    assert_eq!(bytes.len(), MAX_CAPTURED_STREAM_BYTES);
    assert_eq!(bytes.last(), Some(&b'\n'));
}

#[test]
fn directory_size_handles_missing_nested_and_regular_files() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(root.path().join("one"), b"123").unwrap();
    std::fs::write(nested.join("two"), b"4567").unwrap();
    assert_eq!(directory_size(root.path()), 7);
    assert_eq!(directory_size(&root.path().join("missing")), 0);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.path().join("one"), root.path().join("linked")).unwrap();
        assert_eq!(directory_size(root.path()), 7);
    }
}

#[tokio::test]
#[cfg(unix)]
async fn stdout_event_capture_keeps_only_the_latest_bounded_live_events() {
    use tokio::process::Command;

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "i=0; while [ $i -lt {} ]; do printf '{{\"type\":\"run_started\",\"content\":\"%s\"}}\\n' \"$i\"; i=$((i+1)); done",
            MAX_CAPTURED_EVENTS + 1
        ))
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (_, events) = read_stdout_events(stdout, None).await.unwrap();
    child.wait().await.unwrap();

    assert_eq!(events.len(), MAX_CAPTURED_EVENTS);
    assert_eq!(events.first().unwrap().content, "1");
}

#[test]
fn persisted_argument_flags_keep_prefixes_without_values() {
    let flags = persisted_arg_flags(&[
        "--stdio".to_string(),
        "--model=gpt-5".to_string(),
        "-v".to_string(),
        "secret-prompt".to_string(),
    ]);
    assert_eq!(flags, vec!["--stdio", "--model", "-v"]);
}

#[test]
fn executor_run_retention_prunes_oldest_completed_directories() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..=(MAX_RETAINED_RUNS + 1) {
        let run = root.path().join(format!("run-{index:03}"));
        std::fs::create_dir_all(&run).unwrap();
        drop(acquire_external_cli_lock(&run.join(EXTERNAL_CLI_RUN_LOCK_FILE)).unwrap());
        std::fs::write(run.join("result.json"), b"{}").unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    prune_completed_run_directories(root.path(), Some("run-065")).unwrap();
    assert!(!root.path().join("run-000").exists());
    assert!(root.path().join("run-065").exists());
}

#[test]
fn executor_run_retention_prunes_failed_directories_without_result() {
    let root = tempfile::tempdir().unwrap();
    let incomplete = root.path().join("incomplete-active");
    std::fs::create_dir_all(&incomplete).unwrap();
    drop(acquire_external_cli_lock(&incomplete.join(EXTERNAL_CLI_RUN_LOCK_FILE)).unwrap());
    std::fs::write(incomplete.join("cli.stdout.log"), vec![0_u8; 1024]).unwrap();
    for index in 0..=(MAX_RETAINED_RUNS + 1) {
        let run = root.path().join(format!("completed-{index:03}"));
        std::fs::create_dir_all(&run).unwrap();
        drop(acquire_external_cli_lock(&run.join(EXTERNAL_CLI_RUN_LOCK_FILE)).unwrap());
        std::fs::write(run.join("result.json"), b"{}").unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    prune_completed_run_directories(root.path(), None).unwrap();
    assert!(!incomplete.exists());
}

#[test]
fn executor_run_retention_preserves_cross_process_locked_directory() {
    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("active-without-result");
    std::fs::create_dir_all(&active).unwrap();
    let _active_lock = acquire_external_cli_lock(&active.join(EXTERNAL_CLI_RUN_LOCK_FILE)).unwrap();
    std::fs::write(active.join("cli.stdout.log"), vec![0_u8; 1024]).unwrap();
    for index in 0..=(MAX_RETAINED_RUNS + 1) {
        let run = root.path().join(format!("completed-{index:03}"));
        std::fs::create_dir_all(&run).unwrap();
        drop(acquire_external_cli_lock(&run.join(EXTERNAL_CLI_RUN_LOCK_FILE)).unwrap());
        std::fs::write(run.join("result.json"), b"{}").unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }

    prune_completed_run_directories(root.path(), None).unwrap();

    assert!(active.exists());
}

#[test]
fn inactive_run_lock_remains_held_until_pruning_finishes() {
    let root = tempfile::tempdir().unwrap();
    let run = root.path().join("completed");
    std::fs::create_dir_all(&run).unwrap();
    let lock_path = run.join(EXTERNAL_CLI_RUN_LOCK_FILE);
    let initial_lock = acquire_external_cli_lock(&lock_path).unwrap();
    assert!(lock_inactive_external_cli_run(&run).is_none());

    drop(initial_lock);
    let inactive_lock = lock_inactive_external_cli_run(&run).unwrap();
    assert!(lock_inactive_external_cli_run(&run).is_none());
    drop(inactive_lock);
    assert!(lock_inactive_external_cli_run(&run).is_some());
}

#[test]
fn thread_derivation_capability_matrix_reserves_claude_extension_point() {
    assert_eq!(
        thread_derivation_capability("codex", ExternalCliTransport::AppServer),
        ThreadDerivationCapability {
            fork_completed: true,
            fork_active: true,
            fork_at_turn: true,
        }
    );
    assert_eq!(
        thread_derivation_capability(TRAEX_ADAPTER, ExternalCliTransport::AppServer),
        ThreadDerivationCapability {
            fork_completed: true,
            fork_active: false,
            fork_at_turn: false,
        }
    );
    assert_eq!(
        thread_derivation_capability(CLAUDE_CODE_ADAPTER, ExternalCliTransport::StreamJson),
        ThreadDerivationCapability::default()
    );
    assert_eq!(
        thread_derivation_capability(CLAUDE_CODE_ADAPTER, ExternalCliTransport::AppServer),
        ThreadDerivationCapability::default()
    );
    assert_eq!(
        thread_derivation_capability("codex", ExternalCliTransport::Exec),
        ThreadDerivationCapability::default()
    );
    assert_eq!(
        thread_derivation_capability("custom-runner", ExternalCliTransport::AppServer),
        ThreadDerivationCapability::default()
    );
}

#[tokio::test]
async fn external_cli_tee_persists_full_stream_while_forwarding_tail_parser_input() {
    let temp = tempfile::tempdir().unwrap();
    let log_path = temp.path().join("stdout.log");
    let (mut input_tx, input_rx) = tokio::io::duplex(1024);
    let payload = vec![b'x'; MAX_CAPTURED_STREAM_BYTES * 8];
    let expected = payload.clone();
    let writer = tokio::spawn(async move {
        input_tx.write_all(&payload).await.unwrap();
        input_tx.shutdown().await.unwrap();
    });

    let (mut forwarded, tee) = tee_external_cli_output(input_rx, log_path.clone(), "test")
        .await
        .unwrap();
    let mut forwarded_bytes = Vec::new();
    forwarded.read_to_end(&mut forwarded_bytes).await.unwrap();
    writer.await.unwrap();
    join_external_cli_tee(tee, "test").await.unwrap();

    assert_eq!(forwarded_bytes, expected);
    assert_eq!(std::fs::read(log_path).unwrap(), expected);
}

#[test]
fn external_cli_worker_runtime_path_rejects_escape() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let outside = temp.path().join("outside.json");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&outside, b"{}").unwrap();

    let error = validate_external_cli_worker_runtime_path(&outside, &root).unwrap_err();
    assert!(error.contains("outside"));
}

#[test]
fn external_cli_worker_progress_is_bounded() {
    let event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::Status,
        content: "x".repeat(EXTERNAL_CLI_WORKER_PROGRESS_CONTENT_BYTES * 2),
        title: Some("y".repeat(EXTERNAL_CLI_WORKER_PROGRESS_TITLE_BYTES * 2)),
        raw: serde_json::json!({"payload": "z".repeat(EXTERNAL_CLI_WORKER_PROGRESS_CONTENT_BYTES * 2)}),
    };

    let compacted = compact_external_cli_worker_progress(event);
    assert!(compacted.content.len() <= EXTERNAL_CLI_WORKER_PROGRESS_CONTENT_BYTES + 3);
    assert!(compacted.title.unwrap().len() <= EXTERNAL_CLI_WORKER_PROGRESS_TITLE_BYTES + 3);
    assert_eq!(
        compacted.raw.get("_bifrost_compacted"),
        Some(&serde_json::json!(true))
    );
}

#[test]
fn external_cli_worker_json_spool_enforces_atomicity_limits_and_confinement() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    let path = root.join("request.json");
    let value = serde_json::json!({"request": "ok"});

    write_external_cli_worker_json(&path, &value, 1024).unwrap();
    let restored: serde_json::Value = read_external_cli_worker_json(&path, 1024).unwrap();
    assert_eq!(restored, value);
    assert_eq!(
        validate_external_cli_worker_runtime_path(&path, &root).unwrap(),
        std::fs::canonicalize(&path).unwrap()
    );

    let too_small = root.join("too-small.json");
    let error = write_external_cli_worker_json(&too_small, &value, 2).unwrap_err();
    assert!(error.contains("exceeds configured limit"));
    assert!(!too_small.exists());
    assert!(!root.read_dir().unwrap().flatten().any(|entry| entry
        .file_name()
        .to_string_lossy()
        .contains("too-small.tmp")));

    assert!(read_external_cli_worker_json::<serde_json::Value>(&path, 2)
        .unwrap_err()
        .contains("exceeds limit"));
    let invalid = root.join("invalid.json");
    std::fs::write(&invalid, b"not-json").unwrap();
    assert!(
        read_external_cli_worker_json::<serde_json::Value>(&invalid, 1024)
            .unwrap_err()
            .contains("parse")
    );
    assert!(
        read_external_cli_worker_json::<serde_json::Value>(&root.join("missing.json"), 1024,)
            .unwrap_err()
            .contains("stat")
    );
    assert!(
        validate_external_cli_worker_runtime_path(&path, &root.join("missing-root"))
            .unwrap_err()
            .contains("canonicalize")
    );
    assert!(
        validate_external_cli_worker_runtime_path(&root.join("missing-path"), &root)
            .unwrap_err()
            .contains("canonicalize")
    );

    let duplicate_temp = root.join("duplicate.tmp");
    std::fs::write(&duplicate_temp, b"existing").unwrap();
    assert!(open_private_temp_file(&duplicate_temp)
        .unwrap_err()
        .contains("create"));

    let blocked_parent = root.join("blocked-parent");
    std::fs::write(&blocked_parent, b"not-a-directory").unwrap();
    assert!(
        write_external_cli_worker_json(&blocked_parent.join("request.json"), &value, 1024,)
            .unwrap_err()
            .contains("create")
    );

    let directory_target = root.join("directory-target.json");
    std::fs::create_dir(&directory_target).unwrap();
    let rename_error = write_external_cli_worker_json(&directory_target, &value, 1024).unwrap_err();
    assert!(rename_error.contains("rename"), "{rename_error}");
    assert!(!root.read_dir().unwrap().flatten().any(|entry| entry
        .file_name()
        .to_string_lossy()
        .contains("directory-target.tmp")));
}

#[tokio::test]
async fn external_cli_log_helpers_cover_create_append_quota_and_forward_drop() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("run.log");
    ensure_external_cli_log_exists(&log, b"fallback")
        .await
        .unwrap();
    ensure_external_cli_log_exists(&log, b"must-not-overwrite")
        .await
        .unwrap();
    append_external_cli_log(&log, b"-tail").await.unwrap();
    assert_eq!(tokio::fs::read(&log).await.unwrap(), b"fallback-tail");

    let quota = temp.path().join("quota.log");
    let file = std::fs::File::create(&quota).unwrap();
    file.set_len(EXTERNAL_CLI_COMMAND_LOG_MAX_BYTES).unwrap();
    append_external_cli_log(&quota, b"ignored").await.unwrap();
    assert_eq!(
        std::fs::metadata(&quota).unwrap().len(),
        EXTERNAL_CLI_COMMAND_LOG_MAX_BYTES
    );

    let directory = temp.path().join("directory");
    std::fs::create_dir(&directory).unwrap();
    assert!(open_private_file(&directory, true, false)
        .unwrap_err()
        .contains("open private file"));

    let (mut input_tx, input_rx) = tokio::io::duplex(64);
    let dropped_log = temp.path().join("dropped-forward.log");
    let (forwarded, tee) = tee_external_cli_output(input_rx, dropped_log.clone(), "drop")
        .await
        .unwrap();
    drop(forwarded);
    input_tx.write_all(b"still-persisted").await.unwrap();
    input_tx.shutdown().await.unwrap();
    join_external_cli_tee(tee, "drop").await.unwrap();
    assert_eq!(std::fs::read(dropped_log).unwrap(), b"still-persisted");

    let panicked = tokio::spawn(async {
        panic!("expected tee panic");
        #[allow(unreachable_code)]
        Ok::<(), String>(())
    });
    assert!(join_external_cli_tee(panicked, "panic")
        .await
        .unwrap_err()
        .contains("join external CLI panic tee task failed"));

    let marker = temp.path().join("stop.marker");
    std::fs::write(&marker, b"").unwrap();
    wait_for_stop_marker(marker).await;
}

#[test]
fn queued_worker_guard_removes_only_the_owned_registration() {
    let _registry_guard = external_cli_env_guard();
    let session = format!("guard-{}", uuid::Uuid::new_v4());
    let (cancel_tx, _cancel_rx) = watch::channel(false);
    QUEUED_WORKER_SESSIONS.insert(
        session.clone(),
        QueuedExternalCliWorkerControl {
            queue_id: "newer".to_string(),
            cancel_tx,
        },
    );
    drop(QueuedExternalCliWorkerGuard {
        session_key: None,
        queue_id: "ignored".to_string(),
    });
    drop(QueuedExternalCliWorkerGuard {
        session_key: Some(session.clone()),
        queue_id: "older".to_string(),
    });
    assert!(QUEUED_WORKER_SESSIONS.contains_key(&session));
    drop(QueuedExternalCliWorkerGuard {
        session_key: Some(session.clone()),
        queue_id: "newer".to_string(),
    });
    assert!(!QUEUED_WORKER_SESSIONS.contains_key(&session));
}
