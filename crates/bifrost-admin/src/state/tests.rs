use super::*;
use std::env;
use std::fs;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn failed_size_cleanup_preserves_related_record_data() {
    let fixture = crate::test_support::TrafficCleanupFixture::new();
    fixture.reject_metadata_writes();
    fixture.harness.traffic_db.set_max_db_size_bytes(1);
    fixture.harness.state().cleanup_total_disk_usage_if_needed();
    fixture.assert_preserved();
}

#[test]
fn group_cache_resolution_is_single_flight_and_backs_off_failures() {
    let now = std::time::Instant::now();
    let mut state = GroupCacheResolutionState::default();

    let generation = state.try_begin(now).expect("first attempt should start");
    assert!(!state.is_resolved());
    assert_eq!(state.try_begin(now), None, "second attempt must coalesce");

    let retry_after = state
        .finish(generation, false, now)
        .expect("failure should schedule retry");
    assert_eq!(retry_after, std::time::Duration::from_secs(5));
    assert_eq!(
        state.try_begin(now + std::time::Duration::from_secs(4)),
        None
    );
    assert_eq!(
        state.try_begin(now + std::time::Duration::from_secs(5)),
        Some(generation)
    );

    let retry_after = state
        .finish(generation, false, now + std::time::Duration::from_secs(5))
        .expect("second failure should extend retry");
    assert_eq!(retry_after, std::time::Duration::from_secs(10));
}

#[test]
fn group_cache_resolution_success_stays_resolved_until_invalidated() {
    let now = std::time::Instant::now();
    let mut state = GroupCacheResolutionState::default();
    let generation = state.try_begin(now).expect("attempt should start");

    assert_eq!(state.finish(generation, true, now), None);
    assert!(state.is_resolved());
    assert!(state.is_resolved_or_in_flight());
    assert_eq!(
        state.try_begin(now + std::time::Duration::from_secs(600)),
        None
    );

    state.invalidate();
    assert!(!state.is_resolved_or_in_flight());
    assert_ne!(state.try_begin(now), Some(generation));
}

#[test]
fn stale_group_cache_completion_cannot_change_new_generation() {
    let now = std::time::Instant::now();
    let mut state = GroupCacheResolutionState::default();
    let stale_generation = state.try_begin(now).expect("attempt should start");

    state.invalidate();
    let current_generation = state.try_begin(now).expect("new attempt should start");
    assert_ne!(current_generation, stale_generation);
    assert_eq!(state.finish(stale_generation, true, now), None);
    assert!(!state.is_resolved());
    assert!(state.is_resolved_or_in_flight());
    assert_eq!(state.in_flight_generation, Some(current_generation));
}

#[test]
fn public_group_cache_resolved_setter_clears_retry_state() {
    let dir = create_test_dir();
    let state = isolated_test_state(&dir);
    let generation = state
        .try_begin_group_cache_resolution()
        .expect("resolution should start");
    assert!(state
        .finish_group_cache_resolution(generation, false)
        .is_some());

    state.set_group_cache_resolved();
    assert!(state.is_group_cache_resolved());
    assert_eq!(state.try_begin_group_cache_resolution(), None);

    state.clear_group_cache_resolved();
    assert!(!state.is_group_cache_resolved());
    assert!(state.try_begin_group_cache_resolution().is_some());
    cleanup_test_dir(&dir);
}

fn create_test_dir() -> PathBuf {
    let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = env::temp_dir().join(format!(
        "bifrost_state_test_{}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        counter
    ));
    let _ = fs::create_dir_all(&dir);
    dir
}

fn cleanup_test_dir(dir: &PathBuf) {
    let _ = fs::remove_dir_all(dir);
}

fn isolated_test_state(dir: &std::path::Path) -> AdminState {
    AdminState::new_for_test(
        9915,
        RulesStorage::with_dir(dir.join("rules")).expect("test rules storage"),
    )
}

fn make_remote_invoke_worker(
    harness: &crate::test_support::TestAdminState,
    relay_url: &str,
) -> crate::handlers::remote_invoke::SharedRemoteInvokeWorker {
    let identity =
        crate::remote_invoke::Identity::load_or_create(harness.data_dir()).expect("identity");
    crate::remote_invoke::RemoteInvokeWorker::new(
        crate::remote_invoke::RemoteInvokeConfig {
            relay_url: relay_url.to_string(),
            ..Default::default()
        },
        identity,
        None,
        harness.state(),
        "127.0.0.1",
        0,
    )
}

#[test]
fn remote_invoke_workers_track_single_and_dual_channel_relays() {
    let harness = crate::test_support::TestAdminState::builder().build();
    let state = harness.state();
    let internal_relay_url = bifrost_storage::DEFAULT_REMOTE_BASE_URL.as_str();
    let bytedance = make_remote_invoke_worker(&harness, internal_relay_url);
    let cloud = make_remote_invoke_worker(&harness, "https://sync.example.test");

    state.set_remote_invoke_workers(vec![bytedance.clone(), cloud.clone()]);

    assert_eq!(state.remote_invoke_workers().len(), 2);
    assert!(Arc::ptr_eq(
        &state.remote_invoke_worker().expect("primary worker"),
        &bytedance
    ));
    assert!(state
        .remote_invoke_worker_for_relay_url("https://sync.example.test/")
        .is_some());

    state.stop_remote_invoke_workers_except(&[internal_relay_url.to_string()]);

    assert_eq!(state.remote_invoke_workers().len(), 1);
    assert!(state
        .remote_invoke_worker_for_relay_url("https://sync.example.test")
        .is_none());
    assert!(state
        .remote_invoke_worker_for_relay_url(internal_relay_url)
        .is_some());

    state.upsert_remote_invoke_worker(cloud.clone());

    assert_eq!(state.remote_invoke_workers().len(), 2);
    assert!(Arc::ptr_eq(
        &state.remote_invoke_worker().expect("primary worker"),
        &bytedance
    ));
}

#[test]
fn request_tray_launch_invokes_registered_callback() {
    let dir = create_test_dir();
    let calls = Arc::new(AtomicUsize::new(0));
    let callback_calls = calls.clone();
    let state = AdminState::new_for_test(9915, RulesStorage::with_dir(dir.join("rules")).unwrap())
        .with_tray_launch_callback(Arc::new(move || {
            callback_calls.fetch_add(1, Ordering::SeqCst);
        }));

    assert!(state.request_tray_launch());
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let state_without_callback = AdminState::new_for_test(
        9915,
        RulesStorage::with_dir(dir.join("rules-no-callback")).unwrap(),
    );
    assert!(!state_without_callback.request_tray_launch());

    drop(state);
    drop(state_without_callback);
    cleanup_test_dir(&dir);
}

#[test]
fn super_performance_mode_skips_traffic_record_and_update_persistence() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();
    let state = isolated_test_state(&dir)
        .with_traffic_db_store(store)
        .with_super_performance_mode(true);
    let db_store = state
        .traffic_db_store
        .as_ref()
        .expect("traffic db store")
        .clone();

    let mut record = TrafficRecord::new(
        "super-mode-record".to_string(),
        "GET".to_string(),
        "http://example.test/super".to_string(),
    );
    record.status = 200;
    state.record_traffic(record);
    state.update_traffic_by_id("super-mode-record", |record| {
        record.status = 201;
    });

    assert!(db_store.get_by_id("super-mode-record").is_none());
    cleanup_test_dir(&dir);
}

#[test]
fn lifecycle_helper_program_falls_back_to_existing_argv0_when_current_exe_is_stale() {
    let dir = create_test_dir();
    let helper = dir.join("bifrost-helper");
    fs::write(&helper, b"test").expect("write helper");
    let stale_current = dir.join("stale-current-exe");

    let resolved = resolve_system_proxy_lifecycle_helper_program_from_candidates(
        Ok(stale_current),
        Some(helper.clone().into_os_string()),
        &dir,
    )
    .expect("resolve helper");

    assert_eq!(resolved, helper);
    cleanup_test_dir(&dir);
}

#[test]
fn lifecycle_helper_program_keeps_existing_current_exe() {
    let dir = create_test_dir();
    let current = dir.join("current-bifrost");
    fs::write(&current, b"test").expect("write current");
    let argv0 = dir.join("argv0-bifrost");
    fs::write(&argv0, b"test").expect("write argv0");

    let resolved = resolve_system_proxy_lifecycle_helper_program_from_candidates(
        Ok(current.clone()),
        Some(argv0.into_os_string()),
        &dir,
    )
    .expect("resolve helper");

    assert_eq!(resolved, current);
    cleanup_test_dir(&dir);
}

#[test]
fn reconcile_socket_summary_closes_stale_open_connections() {
    let dir = create_test_dir();
    let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
    let mut state = isolated_test_state(&dir);
    state.traffic_db_store = Some(store.clone());

    let mut record = TrafficRecord::new(
        "stale-open-1".to_string(),
        "CONNECT".to_string(),
        "https://ab.chatgpt.com".to_string(),
    );
    record.status = 200;
    record.is_tunnel = true;
    record.socket_status = Some(SocketStatus {
        is_open: true,
        send_count: 1,
        receive_count: 1,
        send_bytes: 128,
        receive_bytes: 64,
        frame_count: 2,
        close_code: None,
        close_reason: None,
    });
    store.record(record);

    let mut summary = store
        .query(&crate::traffic_db::QueryParams {
            limit: Some(10),
            direction: crate::traffic_db::Direction::Forward,
            ..Default::default()
        })
        .records
        .into_iter()
        .find(|item| item.id == "stale-open-1")
        .expect("summary should exist");

    state.reconcile_socket_summary(&mut summary);

    assert_eq!(summary.ss.as_ref().map(|s| s.is_open), Some(false));

    let persisted = store
        .get_by_id("stale-open-1")
        .expect("record should still exist");
    assert_eq!(
        persisted.socket_status.as_ref().map(|s| s.is_open),
        Some(false)
    );

    cleanup_test_dir(&dir);
}

#[test]
fn reconcile_socket_summary_synthesizes_closed_status_for_missing_socket_state() {
    let dir = create_test_dir();
    let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
    let mut state = isolated_test_state(&dir);
    state.traffic_db_store = Some(store.clone());

    let mut record = TrafficRecord::new(
        "missing-status-1".to_string(),
        "CONNECT".to_string(),
        "https://example.com".to_string(),
    );
    record.status = 200;
    record.is_tunnel = true;
    store.record(record);

    let mut summary = store
        .query(&crate::traffic_db::QueryParams {
            limit: Some(10),
            direction: crate::traffic_db::Direction::Forward,
            ..Default::default()
        })
        .records
        .into_iter()
        .find(|item| item.id == "missing-status-1")
        .expect("summary should exist");

    assert!(summary.ss.is_none());

    state.reconcile_socket_summary(&mut summary);

    assert_eq!(summary.ss.as_ref().map(|s| s.is_open), Some(false));

    cleanup_test_dir(&dir);
}

#[test]
fn reconcile_socket_summary_preserves_sse_counts_when_closing_stale_open_status() {
    let dir = create_test_dir();
    let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
    let mut state = isolated_test_state(&dir);
    state.traffic_db_store = Some(store.clone());

    let mut record = TrafficRecord::new(
        "stale-sse-open-1".to_string(),
        "GET".to_string(),
        "https://example.com/stream".to_string(),
    );
    record.status = 200;
    record.is_sse = true;
    record.content_type = Some("text/event-stream".to_string());
    record.response_size = 4096;
    record.frame_count = 12;
    record.last_frame_id = 12;
    record.socket_status = Some(SocketStatus {
        is_open: true,
        send_count: 0,
        receive_count: 0,
        send_bytes: 0,
        receive_bytes: 0,
        frame_count: 0,
        close_code: None,
        close_reason: None,
    });
    store.record(record);

    let mut summary = store
        .query(&crate::traffic_db::QueryParams {
            limit: Some(10),
            direction: crate::traffic_db::Direction::Forward,
            ..Default::default()
        })
        .records
        .into_iter()
        .find(|item| item.id == "stale-sse-open-1")
        .expect("summary should exist");

    summary.fc = 12;
    summary.res_sz = 4096;

    state.reconcile_socket_summary(&mut summary);

    let socket_status = summary.ss.expect("socket status should exist");
    assert!(!socket_status.is_open);
    assert_eq!(socket_status.frame_count, 12);
    assert_eq!(socket_status.receive_count, 12);
    assert_eq!(socket_status.receive_bytes, 4096);

    let persisted = store
        .get_by_id("stale-sse-open-1")
        .expect("record should still exist");
    let persisted_status = persisted.socket_status.expect("persisted socket status");
    assert!(!persisted_status.is_open);
    assert_eq!(persisted_status.frame_count, 12);
    assert_eq!(persisted_status.receive_count, 12);
    assert_eq!(persisted_status.receive_bytes, 4096);

    cleanup_test_dir(&dir);
}

#[test]
fn reconcile_socket_summary_does_not_persist_empty_sse_close_from_stale_snapshot() {
    let dir = create_test_dir();
    let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
    let mut state = isolated_test_state(&dir);
    state.traffic_db_store = Some(store.clone());

    let mut record = TrafficRecord::new(
        "stale-sse-empty-1".to_string(),
        "GET".to_string(),
        "https://example.com/stream".to_string(),
    );
    record.status = 200;
    record.is_sse = true;
    record.content_type = Some("text/event-stream".to_string());
    record.socket_status = Some(SocketStatus {
        is_open: true,
        send_count: 0,
        receive_count: 0,
        send_bytes: 0,
        receive_bytes: 0,
        frame_count: 0,
        close_code: None,
        close_reason: None,
    });
    store.record(record);

    let mut summary = store
        .query(&crate::traffic_db::QueryParams {
            limit: Some(10),
            direction: crate::traffic_db::Direction::Forward,
            ..Default::default()
        })
        .records
        .into_iter()
        .find(|item| item.id == "stale-sse-empty-1")
        .expect("summary should exist");

    state.reconcile_socket_summary(&mut summary);

    let socket_status = summary.ss.expect("socket status should exist");
    assert!(!socket_status.is_open);
    assert_eq!(socket_status.receive_count, 0);
    assert_eq!(socket_status.receive_bytes, 0);

    let persisted = store
        .get_by_id("stale-sse-empty-1")
        .expect("record should still exist");
    let persisted_status = persisted.socket_status.expect("persisted socket status");
    assert!(persisted_status.is_open);
    assert_eq!(persisted_status.receive_count, 0);
    assert_eq!(persisted_status.receive_bytes, 0);

    cleanup_test_dir(&dir);
}

#[test]
fn group_name_cache_persist_and_load_round_trip() {
    let dir = create_test_dir();
    let rules_dir = dir.join("rules");
    let _ = fs::create_dir_all(&rules_dir);
    let storage = RulesStorage::with_dir(rules_dir.clone()).unwrap();

    let state = AdminState::new_for_test(19900, storage);

    {
        let mut cache = state.group_name_cache();
        cache.insert("g1".to_string(), "GroupAlpha".to_string());
        cache.insert("g2".to_string(), "GroupBeta".to_string());
    }
    state.persist_group_name_cache();

    let cache_file = rules_dir.join(".group_cache.json");
    assert!(cache_file.exists(), "cache file should be written to disk");

    let state2 = AdminState::new_for_test(19901, RulesStorage::with_dir(rules_dir).unwrap());
    state2.load_group_name_cache();

    {
        let cache = state2.group_name_cache();
        assert_eq!(cache.get("g1"), Some("GroupAlpha".to_string()));
        assert_eq!(cache.get("g2"), Some("GroupBeta".to_string()));
        assert_eq!(cache.reverse_lookup("GroupAlpha"), Some("g1".to_string()));
    }

    cleanup_test_dir(&dir);
}

#[test]
fn group_name_cache_load_missing_file_is_noop() {
    let dir = create_test_dir();
    let rules_dir = dir.join("rules");
    let _ = fs::create_dir_all(&rules_dir);
    let storage = RulesStorage::with_dir(rules_dir).unwrap();

    let state = AdminState::new_for_test(19902, storage);

    state.load_group_name_cache();
    let cache = state.group_name_cache();
    assert_eq!(cache.get("any"), None);

    cleanup_test_dir(&dir);
}

#[test]
fn group_name_cache_load_corrupt_file_is_ignored() {
    let dir = create_test_dir();
    let rules_dir = dir.join("rules");
    let _ = fs::create_dir_all(&rules_dir);

    fs::write(rules_dir.join(".group_cache.json"), "not json!!!").unwrap();

    let storage = RulesStorage::with_dir(rules_dir).unwrap();
    let state = AdminState::new_for_test(19903, storage);

    state.load_group_name_cache();
    let cache = state.group_name_cache();
    assert_eq!(cache.get("any"), None);

    cleanup_test_dir(&dir);
}

#[test]
fn group_name_cache_persist_empty_is_noop() {
    let dir = create_test_dir();
    let rules_dir = dir.join("rules");
    let _ = fs::create_dir_all(&rules_dir);
    let storage = RulesStorage::with_dir(rules_dir.clone()).unwrap();

    let state = AdminState::new_for_test(19904, storage);

    state.persist_group_name_cache();
    assert!(
        !rules_dir.join(".group_cache.json").exists(),
        "empty cache should not create file"
    );

    cleanup_test_dir(&dir);
}

#[test]
fn badge_rules_cache_preserves_group_navigation_mapping() {
    let dir = create_test_dir();
    let rules_dir = dir.join("rules");
    let group_dir = rules_dir.join("TeamAlpha");
    let _ = fs::create_dir_all(&group_dir);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let config_manager = Arc::new(ConfigManager::new(dir.join("config")).unwrap());
    let sync_manager = Arc::new(bifrost_sync::SyncManager::new(config_manager, 19903).unwrap());
    runtime
        .block_on(sync_manager.save_token("badge-test-token".to_string()))
        .unwrap();
    let storage = RulesStorage::with_dir(rules_dir).unwrap();
    let group_storage = RulesStorage::with_dir(group_dir).unwrap();

    let mut group_rule =
        bifrost_storage::RuleFile::new("team-rule", "team.example.com status://200");
    group_rule.enabled = true;
    group_rule.group = Some("TeamAlpha".to_string());
    group_storage.save(&group_rule).unwrap();

    let state = AdminState::new_for_test(19903, storage).with_sync_manager_shared(sync_manager);
    {
        let mut cache = state.group_name_cache();
        cache.insert("gid-alpha".to_string(), "TeamAlpha".to_string());
    }

    state.refresh_badge_rules_cache();
    let json: serde_json::Value = serde_json::from_str(&state.badge_rules_json()).unwrap();
    let rule = json["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "team-rule")
        .expect("badge rule should exist");

    assert_eq!(rule["group_id"], "TeamAlpha");
    assert_eq!(rule["group_name"], "gid-alpha");

    cleanup_test_dir(&dir);
}

#[test]
fn badge_rules_cache_includes_share_env_state() {
    let dir = create_test_dir();
    let rules_dir = dir.join("rules");
    let _ = fs::create_dir_all(&rules_dir);
    let storage = RulesStorage::with_dir(rules_dir).unwrap();
    storage
        .save(&bifrost_storage::RuleFile::new(
            "share/demo",
            "demo.example.com statusCode://204",
        ))
        .unwrap();
    storage
        .save_share_env_state(&bifrost_storage::ShareEnvState {
            active: true,
            imported_rule_name: "share/demo".to_string(),
            requested_name: "demo".to_string(),
            content_hash: "hash".to_string(),
            enabled_rule_names: vec!["before".to_string()],
            entered_at: "2026-06-22T00:00:00Z".to_string(),
            exit_token: "exit-token".to_string(),
        })
        .unwrap();

    let state = AdminState::new_for_test(19905, storage);
    state.refresh_badge_rules_cache();
    let json: serde_json::Value = serde_json::from_str(&state.badge_rules_json()).unwrap();

    assert_eq!(json["share_env"]["active"], true);
    assert_eq!(json["share_env"]["requested_name"], "demo");
    assert_eq!(json["share_env"]["imported_rule_name"], "share/demo");
    assert_eq!(json["share_env"]["exit_token"], "exit-token");
    assert!(json["share_env"].get("enabled_rule_names").is_none());

    cleanup_test_dir(&dir);
}
