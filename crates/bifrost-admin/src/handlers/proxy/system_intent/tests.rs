use super::*;
use crate::state::AdminState;
use http_body_util::BodyExt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn observed(enabled: bool) -> SystemProxyStatus {
    SystemProxyStatus::from_proxy(
        bifrost_core::ProxyBackup {
            enable: enabled,
            host: "127.0.0.1".to_string(),
            port: 18891,
            bypass: "observed.example".to_string(),
        },
        enabled,
    )
}

fn request(enabled: bool) -> SetSystemProxyRequest {
    SetSystemProxyRequest {
        enabled,
        bypass: Some("requested.example".to_string()),
        recovery_mode: Some(SystemProxyRecoveryMode::FailClosed),
        recovery_grace_secs: Some(4),
    }
}

async fn setup(enabled: bool) -> (tempfile::TempDir, SharedAdminState, Arc<AtomicBool>) {
    let temp = tempfile::tempdir().unwrap();
    let config = bifrost_storage::ConfigManager::new(temp.path().to_path_buf()).unwrap();
    config
        .update_system_proxy_config(SystemProxyConfigUpdate {
            enabled: Some(enabled),
            ..Default::default()
        })
        .await
        .unwrap();
    let desired = Arc::new(AtomicBool::new(enabled));
    let state = AdminState::new_for_test(
        18891,
        bifrost_storage::RulesStorage::with_dir(temp.path().join("rules")).unwrap(),
    )
    .with_config_manager(config)
    .with_system_proxy_manager(SystemProxyManager::new(temp.path().to_path_buf()))
    .with_system_proxy_runtime_flags_shared(desired.clone(), Arc::new(AtomicBool::new(enabled)));
    (temp, Arc::new(state), desired)
}

async fn apply_mock(
    operation: SystemProxyOperation,
    result: Result<(), String>,
) -> Result<(), String> {
    run_system_proxy_worker("mock", move || {
        apply_system_proxy_operation_locked(operation, |_, _| result)
    })
    .await
}

async fn body(response: Response<BoxBody>) -> serde_json::Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn false_readback_after_enable_keeps_durable_desired_and_reports_mismatch() {
    let (temp, state, desired) = setup(false).await;
    let response = apply_system_proxy_intent_with(
        state.clone(),
        request(true),
        |operation| apply_mock(operation, Ok(())),
        |enabled, port| async move {
            wait_for_system_proxy_status_with(enabled, "127.0.0.1", port, &[], || async {
                Ok(observed(false))
            })
            .await
        },
        |_, _| panic!("mismatch is not success"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = body(response).await;
    assert_eq!(body["configured_enabled"], true);
    assert_eq!(body["effective_status"]["enabled"], false);
    assert!(body["error"].as_str().unwrap().contains("did not converge"));
    assert!(desired.load(Ordering::Acquire));
    let saved = bifrost_storage::ConfigManager::new(temp.path().to_path_buf())
        .unwrap()
        .config()
        .await
        .system_proxy;
    assert!(saved.enabled);
    assert_eq!(saved.bypass, "requested.example");
    assert_eq!(saved.recovery_mode, SystemProxyRecoveryMode::FailClosed);
    assert_eq!(saved.recovery_grace_secs, 4);
}

#[tokio::test]
async fn transient_os_failure_does_not_rollback_an_accepted_toggle() {
    for enabled in [true, false] {
        let (_temp, state, desired) = setup(!enabled).await;
        let response = apply_system_proxy_intent_with(
            state.clone(),
            request(enabled),
            |operation| apply_mock(operation, Err("temporary OS error".to_string())),
            |_, _| async {
                Err(SystemProxyVerificationError {
                    message: "read failed".to_string(),
                    status: None,
                })
            },
            |_, _| panic!("OS failure is not success"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body(response).await;
        assert_eq!(body["configured_enabled"], enabled);
        assert!(body["effective_status"].is_null());
        assert_eq!(desired.load(Ordering::Acquire), enabled);
        assert_eq!(
            state
                .config_manager
                .as_ref()
                .unwrap()
                .config()
                .await
                .system_proxy
                .enabled,
            enabled
        );
    }
}

#[tokio::test]
async fn failed_persistence_has_no_runtime_or_os_side_effects() {
    let (temp, state, desired) = setup(false).await;
    let before = state
        .config_manager
        .as_ref()
        .unwrap()
        .config()
        .await
        .system_proxy;
    std::fs::remove_file(temp.path().join("config.toml")).unwrap();
    std::fs::create_dir(temp.path().join("config.toml")).unwrap();
    let response = apply_system_proxy_intent_with(
        state.clone(),
        request(true),
        |_| async { panic!("must not write OS before persistence") },
        |_, _| async { panic!("must not read back an unaccepted change") },
        |_, _| {},
    )
    .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = body(response).await;
    assert_eq!(body["configured_enabled"], false);
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("persist system proxy intent"));
    assert!(!desired.load(Ordering::Acquire));
    let after = state
        .config_manager
        .as_ref()
        .unwrap()
        .config()
        .await
        .system_proxy;
    assert!(!after.enabled);
    assert_eq!(before.intent_revision, after.intent_revision);
}

#[tokio::test]
async fn concurrent_enable_then_disable_is_serialized_through_readback() {
    let (_temp, state, desired) = setup(false).await;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let first_state = state.clone();
    let first = tokio::spawn(async move {
        apply_system_proxy_intent_with(
            first_state,
            request(true),
            |operation| async {
                run_system_proxy_worker("mock", move || {
                    apply_system_proxy_operation_locked(operation, |_, _| {
                        started_tx.send(()).unwrap();
                        release_rx.blocking_recv().unwrap();
                        Err("late enable failed".to_string())
                    })
                })
                .await
            },
            |_, _| async { Ok(observed(false)) },
            |_, _| {},
        )
        .await
    });
    started_rx.await.unwrap();
    let second_state = state.clone();
    let second = tokio::spawn(async move {
        apply_system_proxy_intent_with(
            second_state,
            request(false),
            |operation| apply_mock(operation, Ok(())),
            |_, _| async { Ok(observed(false)) },
            |_, _| {},
        )
        .await
    });
    tokio::task::yield_now().await;
    assert!(desired.load(Ordering::Acquire));
    assert!(!second.is_finished());
    release_tx.send(()).unwrap();
    assert_eq!(
        first.await.unwrap().status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let response = second.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await["configured_enabled"], false);
    assert!(!desired.load(Ordering::Acquire));
    let saved = state
        .config_manager
        .as_ref()
        .unwrap()
        .config()
        .await
        .system_proxy;
    assert!(!saved.enabled);
    assert_eq!(saved.intent_revision, 3);
}

#[tokio::test]
async fn omitted_bypass_preserves_saved_value_without_adopting_os_readback() {
    let (_temp, state, _) = setup(false).await;
    let before = state
        .config_manager
        .as_ref()
        .unwrap()
        .config()
        .await
        .system_proxy
        .bypass;
    let mut request = request(true);
    request.bypass = None;
    let expected = before.clone();
    let response = apply_system_proxy_intent_with(
        state.clone(),
        request,
        move |operation| async move {
            assert_eq!(operation.bypass, expected);
            apply_mock(operation, Ok(())).await
        },
        |_, _| async { Ok(observed(true)) },
        |_, _| {},
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await["configured_bypass"], before);
    assert_eq!(
        state
            .config_manager
            .as_ref()
            .unwrap()
            .config()
            .await
            .system_proxy
            .bypass,
        before
    );
}

#[tokio::test]
async fn verification_retries_errors_and_mismatches_until_converged() {
    let mut samples = vec![
        Err("transient read error".to_string()),
        Ok(observed(false)),
        Ok(observed(true)),
    ]
    .into_iter();
    let status = wait_for_system_proxy_status_with(true, "127.0.0.1", 18891, &[0, 0], || {
        std::future::ready(samples.next().unwrap())
    })
    .await
    .unwrap();
    assert!(status.enabled);
}

#[tokio::test]
async fn verification_exhaustion_includes_last_known_effective_state() {
    let mut samples = vec![Ok(observed(true)), Err("last read error".to_string())].into_iter();
    let error = wait_for_system_proxy_status_with(false, "127.0.0.1", 18891, &[0], || {
        std::future::ready(samples.next().unwrap())
    })
    .await
    .unwrap_err();
    assert_eq!(error.message, "last read error");
    assert!(error.status.unwrap().enabled);
}

#[tokio::test]
async fn status_reads_latest_persisted_intent_from_another_config_manager() {
    let (temp, state, _) = setup(true).await;
    let other = bifrost_storage::ConfigManager::new(temp.path().to_path_buf()).unwrap();
    other
        .update_system_proxy_config(SystemProxyConfigUpdate {
            enabled: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        state
            .config_manager
            .as_ref()
            .unwrap()
            .config()
            .await
            .system_proxy
            .enabled
    );
    assert!(!current_system_proxy_config(&state).await.enabled);
    std::fs::write(temp.path().join("config.toml"), "[invalid").unwrap();
    // An unreadable file is not a fabricated disable.
    assert!(current_system_proxy_config(&state).await.enabled);
}

#[tokio::test]
async fn newer_disable_supersedes_enable_waiting_for_manager_lock() {
    let (temp, state, desired) = setup(false).await;
    let manager_guard = state.system_proxy_manager.as_ref().unwrap().write().await;
    let (queued_tx, queued_rx) = tokio::sync::oneshot::channel();
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        apply_system_proxy_intent_with(
            task_state,
            request(true),
            |operation| async move {
                queued_tx.send(()).unwrap();
                run_system_proxy_worker("mock", move || {
                    apply_system_proxy_operation_locked(operation, |_, _| {
                        panic!("superseded request must not mutate OS")
                    })
                })
                .await
            },
            |_, _| async { Ok(observed(false)) },
            |_, _| panic!("superseded request is not success"),
        )
        .await
    });
    queued_rx.await.unwrap();
    let other = bifrost_storage::ConfigManager::new(temp.path().to_path_buf()).unwrap();
    other
        .update_system_proxy_config(SystemProxyConfigUpdate {
            enabled: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!desired.load(Ordering::Acquire));
    drop(manager_guard);
    let response = task.await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = body(response).await;
    assert_eq!(body["configured_enabled"], false);
    assert!(body["error"].as_str().unwrap().contains("superseded"));
    assert!(!desired.load(Ordering::Acquire));
}

#[tokio::test]
async fn queued_request_uses_current_port_under_manager_lock() {
    let (_temp, state, _) = setup(false).await;
    let task_state = state.clone();
    let response = apply_system_proxy_intent_with(
        state.clone(),
        request(true),
        |operation| async move {
            // Simulate the rebind committing before this queued operation takes its lock.
            {
                let _guard = task_state
                    .system_proxy_manager
                    .as_ref()
                    .unwrap()
                    .write()
                    .await;
                task_state.set_port(18892);
            }
            run_system_proxy_worker("mock", move || {
                apply_system_proxy_operation_locked(operation, |_, port| {
                    assert_eq!(port, 18892);
                    Ok(())
                })
            })
            .await
        },
        |_, port| async move {
            assert_eq!(port, 18892);
            let mut status = observed(true);
            status.port = port;
            Ok(status)
        },
        |_, _| {},
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await["port"], 18892);
}

#[tokio::test]
async fn rebind_during_readback_does_not_report_success_for_retired_target() {
    let (_temp, state, _) = setup(false).await;
    let verifier_state = state.clone();
    let response = apply_system_proxy_intent_with(
        state,
        request(true),
        |operation| apply_mock(operation, Ok(())),
        |_, _| async move {
            let _guard = verifier_state
                .system_proxy_manager
                .as_ref()
                .unwrap()
                .write()
                .await;
            verifier_state.set_port(18892);
            Ok(observed(true))
        },
        |_, _| panic!("retired target must not report success"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body(response).await["error"]
        .as_str()
        .unwrap()
        .contains("target changed"));
}

#[tokio::test]
async fn newer_disable_during_enable_gets_disable_only_compensation() {
    for requested_enabled in [true, false] {
        let (temp, state, desired) = setup(requested_enabled).await;
        let other = bifrost_storage::ConfigManager::new(temp.path().to_path_buf()).unwrap();
        let compensated = Arc::new(AtomicBool::new(false));
        let compensation_flag = compensated.clone();
        let operation = SystemProxyOperation {
            state: state.clone(),
            enabled: requested_enabled,
            bypass: String::new(),
            intent_revision: 1,
        };
        let error = run_system_proxy_worker("mock", move || {
            apply_system_proxy_operation_locked_with(
                operation,
                |_, _| {
                    tokio::runtime::Handle::current()
                        .block_on(other.update_system_proxy_config(SystemProxyConfigUpdate {
                            enabled: Some(!requested_enabled),
                            ..Default::default()
                        }))
                        .unwrap();
                    Ok(())
                },
                |_, port| {
                    assert_eq!(port, 18891);
                    compensation_flag.store(true, Ordering::Release);
                    Ok(())
                },
            )
        })
        .await
        .unwrap_err();
        assert!(error.contains("superseded while applying"));
        assert_eq!(desired.load(Ordering::Acquire), !requested_enabled);
        assert_eq!(compensated.load(Ordering::Acquire), requested_enabled);
    }
}

#[tokio::test]
async fn accepted_intent_is_rechecked_after_waiting_for_simulated_os_lock() {
    for requested in [true, false] {
        let (temp, state, desired) = setup(!requested).await;
        let (os_release_tx, os_release_rx) = tokio::sync::oneshot::channel();
        let mutation_ran = Arc::new(AtomicBool::new(false));
        let mutation_flag = mutation_ran.clone();
        let (queued_tx, queued_rx) = tokio::sync::oneshot::channel();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            apply_system_proxy_intent_with(
                task_state,
                request(requested),
                |operation| async move {
                    let guard_state = operation.state.clone();
                    let revision = operation.intent_revision;
                    run_system_proxy_worker("mock OS lock", move || {
                        apply_system_proxy_operation_locked_with(
                            operation,
                            |_, port| {
                                queued_tx.send(()).unwrap();
                                os_release_rx.blocking_recv().unwrap();
                                // This is the same predicate passed to each core direct
                                // and GUI method, evaluated after that method's OS lock.
                                if !accepted_system_proxy_intent_is_current(
                                    &guard_state,
                                    revision,
                                    requested,
                                    port,
                                )
                                .map_err(|error| error.to_string())?
                                {
                                    return Err("superseded inside OS lock".to_string());
                                }
                                mutation_flag.store(true, Ordering::Release);
                                Ok(())
                            },
                            |_, _| Ok(()),
                        )
                    })
                    .await
                },
                |_, _| async move { Ok(observed(!requested)) },
                |_, _| panic!("stale request is not success"),
            )
            .await
        });
        queued_rx.await.unwrap();
        let other = bifrost_storage::ConfigManager::new(temp.path().to_path_buf()).unwrap();
        other
            .update_system_proxy_config(SystemProxyConfigUpdate {
                enabled: Some(!requested),
                ..Default::default()
            })
            .await
            .unwrap();
        os_release_tx.send(()).unwrap();
        let response = task.await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!mutation_ran.load(Ordering::Acquire));
        assert_eq!(desired.load(Ordering::Acquire), !requested);
        assert_eq!(body(response).await["configured_enabled"], !requested);
    }
}

#[tokio::test]
async fn compensation_rechecks_latest_off_intent_after_waiting_for_os_lock() {
    let (temp, state, _) = setup(true).await;
    let other = bifrost_storage::ConfigManager::new(temp.path().to_path_buf()).unwrap();
    let guard_state = state.clone();
    let operation = SystemProxyOperation {
        state,
        enabled: true,
        bypass: String::new(),
        intent_revision: 1,
    };
    let (os_release_tx, os_release_rx) = tokio::sync::oneshot::channel();
    let compensated = Arc::new(AtomicBool::new(false));
    let compensation_flag = compensated.clone();
    let (queued_tx, queued_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        run_system_proxy_worker("mock compensation lock", move || {
            apply_system_proxy_operation_locked_with(
                operation,
                |_, _| {
                    tokio::runtime::Handle::current()
                        .block_on(other.update_system_proxy_config(SystemProxyConfigUpdate {
                            enabled: Some(false),
                            ..Default::default()
                        }))
                        .unwrap();
                    Ok(())
                },
                |_, port| {
                    queued_tx.send(()).unwrap();
                    os_release_rx.blocking_recv().unwrap();
                    if newer_system_proxy_disable_is_current(&guard_state, 1, port)
                        .map_err(|error| error.to_string())?
                    {
                        compensation_flag.store(true, Ordering::Release);
                    }
                    Ok(())
                },
            )
        })
        .await
    });
    queued_rx.await.unwrap();
    let newest = bifrost_storage::ConfigManager::new(temp.path().to_path_buf()).unwrap();
    newest
        .update_system_proxy_config(SystemProxyConfigUpdate {
            enabled: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    os_release_tx.send(()).unwrap();
    assert!(task.await.unwrap().is_err());
    assert!(!compensated.load(Ordering::Acquire));
}

#[tokio::test]
async fn mutation_guards_reject_unknown_intent_and_changed_target() {
    let (temp, state, _) = setup(true).await;
    assert!(accepted_system_proxy_intent_is_current(&state, 1, true, 18891).unwrap());
    assert!(!accepted_system_proxy_intent_is_current(&state, 1, true, 18892).unwrap());
    std::fs::write(temp.path().join("config.toml"), "[invalid").unwrap();
    assert!(accepted_system_proxy_intent_is_current(&state, 1, true, 18891).is_err());
    assert!(newer_system_proxy_disable_is_current(&state, 0, 18891).is_err());
}

async fn assert_missing_dependency_rejects_without_side_effects(missing_proxy_manager: bool) {
    let (temp, mut state, desired) = setup(false).await;
    let config_path = temp.path().join("config.toml");
    let persisted_before = std::fs::read(&config_path).unwrap();
    let expected_error = if missing_proxy_manager {
        Arc::get_mut(&mut state).unwrap().system_proxy_manager = None;
        "System proxy manager not initialized"
    } else {
        Arc::get_mut(&mut state).unwrap().config_manager = None;
        "Config manager not available"
    };
    let mutation_called = AtomicBool::new(false);
    let verification_called = AtomicBool::new(false);
    let completion_called = AtomicBool::new(false);
    let response = apply_system_proxy_intent_with(
        state.clone(),
        request(true),
        |_| {
            mutation_called.store(true, Ordering::Release);
            std::future::ready(Ok(()))
        },
        |_, _| {
            verification_called.store(true, Ordering::Release);
            std::future::ready(Ok(observed(true)))
        },
        |_, _| completion_called.store(true, Ordering::Release),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body(response).await["error"], expected_error);
    assert!(!mutation_called.load(Ordering::Acquire));
    assert!(!verification_called.load(Ordering::Acquire));
    assert!(!completion_called.load(Ordering::Acquire));
    assert!(!desired.load(Ordering::Acquire));
    assert_eq!(std::fs::read(&config_path).unwrap(), persisted_before);

    // The lower-level worker must reject the same incomplete state too, before
    // taking a native mutation or compensation path.
    let operation = SystemProxyOperation {
        state,
        enabled: true,
        bypass: "requested.example".to_string(),
        intent_revision: 1,
    };
    let error = run_system_proxy_worker("missing dependency", move || {
        apply_system_proxy_operation_locked_with(
            operation,
            |_, _| panic!("missing dependency must not reach mutation"),
            |_, _| panic!("missing dependency must not reach compensation"),
        )
    })
    .await
    .unwrap_err();
    assert_eq!(error, expected_error);
    assert_eq!(std::fs::read(config_path).unwrap(), persisted_before);
}

#[tokio::test]
async fn missing_proxy_manager_returns_unavailable_before_any_side_effects() {
    assert_missing_dependency_rejects_without_side_effects(true).await;
}

#[tokio::test]
async fn missing_config_manager_returns_unavailable_before_any_side_effects() {
    assert_missing_dependency_rejects_without_side_effects(false).await;
}

#[tokio::test]
async fn missing_config_manager_rejects_both_os_write_predicates() {
    let (_temp, mut state, _) = setup(false).await;
    Arc::get_mut(&mut state).unwrap().config_manager = None;
    for error in [
        accepted_system_proxy_intent_is_current(&state, 1, false, 18891).unwrap_err(),
        newer_system_proxy_disable_is_current(&state, 0, 18891).unwrap_err(),
    ] {
        assert!(error.to_string().contains("Config manager not available"));
    }
}

#[tokio::test]
async fn newer_disable_predicate_rejects_a_retired_target() {
    let (_temp, state, _) = setup(true).await;
    state
        .config_manager
        .as_ref()
        .unwrap()
        .update_system_proxy_config(SystemProxyConfigUpdate {
            enabled: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    {
        let _guard = state.system_proxy_manager.as_ref().unwrap().write().await;
        state.set_port(18892);
    }
    assert!(!newer_system_proxy_disable_is_current(&state, 1, 18891).unwrap());
    assert!(newer_system_proxy_disable_is_current(&state, 1, 18892).unwrap());
}

#[tokio::test]
async fn failed_readback_after_successful_mock_write_retains_intent_without_completion() {
    let (temp, state, desired) = setup(false).await;
    let completed = AtomicBool::new(false);
    let response = apply_system_proxy_intent_with(
        state,
        request(true),
        |operation| async move {
            run_system_proxy_worker("fake mutation", move || {
                apply_system_proxy_operation_locked_with(
                    operation,
                    |_, _| Ok(()),
                    |_, _| panic!("unchanged intent must not need compensation"),
                )
            })
            .await
        },
        |_, _| async {
            Err(SystemProxyVerificationError {
                message: "readback unavailable".to_string(),
                status: None,
            })
        },
        |_, _| completed.store(true, Ordering::Release),
    )
    .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let response = body(response).await;
    assert!(response["error"]
        .as_str()
        .unwrap()
        .contains("Failed to verify system proxy: readback unavailable"));
    assert_eq!(response["configured_enabled"], true);
    assert!(response["effective_status"].is_null());
    assert!(desired.load(Ordering::Acquire));
    assert!(!completed.load(Ordering::Acquire));
    let persisted = bifrost_storage::read_persisted_system_proxy_config(temp.path()).unwrap();
    assert!(persisted.enabled);
    assert_eq!(persisted.intent_revision, 2);
}

#[tokio::test]
async fn compensation_without_an_ownership_journal_does_not_create_one() {
    let (temp, state, desired) = setup(true).await;
    let other = bifrost_storage::ConfigManager::new(temp.path().to_path_buf()).unwrap();
    let journal_path = temp.path().join("proxy_state.json");
    assert!(!journal_path.exists());
    assert!(!temp.path().join("runtime.json").exists());
    let operation = SystemProxyOperation {
        state,
        enabled: true,
        bypass: "requested.example".to_string(),
        intent_revision: 1,
    };
    let error = run_system_proxy_worker("missing ownership compensation", move || {
        // Mutation is fake. The real compensation wrapper sees no lease in the
        // fresh tempdir and therefore cannot invoke native proxy restoration.
        apply_system_proxy_operation_locked(operation, |_, _| {
            tokio::runtime::Handle::current()
                .block_on(other.update_system_proxy_config(SystemProxyConfigUpdate {
                    enabled: Some(false),
                    ..Default::default()
                }))
                .unwrap();
            Ok(())
        })
    })
    .await
    .unwrap_err();
    assert!(error.contains("superseded while applying OS settings"));
    assert!(!desired.load(Ordering::Acquire));
    assert!(!journal_path.exists());
    assert!(!temp.path().join("runtime.json").exists());
    assert!(
        !bifrost_storage::read_persisted_system_proxy_config(temp.path())
            .unwrap()
            .enabled
    );
}
