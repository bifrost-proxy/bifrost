use super::*;
use tempfile::TempDir;

fn setup() -> (TempDir, ConfigManager) {
    let temp_dir = TempDir::new().unwrap();
    let manager = ConfigManager::new(temp_dir.path().to_path_buf()).unwrap();
    (temp_dir, manager)
}

#[tokio::test]
async fn test_config_manager_new() {
    let (_temp_dir, manager) = setup();
    let config = manager.config().await;

    assert_eq!(config.server.timeout_secs, 30);
    assert!(!config.tls.enable_interception);
}

#[tokio::test]
async fn test_missing_expected_data_subdirs_are_recreated() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::create_dir_all(temp_dir.path()).unwrap();

    for subdir in ["rules", "body_cache"] {
        std::fs::create_dir_all(temp_dir.path().join(subdir)).unwrap();
    }

    let _manager = ConfigManager::new(temp_dir.path().to_path_buf()).unwrap();

    for subdir in ConfigManager::expected_data_subdirs() {
        assert!(
            temp_dir.path().join(subdir).is_dir(),
            "expected subdir to exist: {subdir}"
        );
    }
}

#[tokio::test]
async fn test_update_tls_config() {
    let (_temp_dir, manager) = setup();

    let update = TlsConfigUpdate {
        enable_interception: Some(false),
        unsafe_ssl: Some(true),
        ..Default::default()
    };

    manager.update_tls_config(update).await.unwrap();

    let config = manager.config().await;
    assert!(!config.tls.enable_interception);
    assert!(config.tls.unsafe_ssl);
}

#[tokio::test]
async fn test_config_persistence() {
    let temp_dir = TempDir::new().unwrap();

    {
        let manager = ConfigManager::new(temp_dir.path().to_path_buf()).unwrap();
        let update = TlsConfigUpdate {
            enable_interception: Some(false),
            ..Default::default()
        };
        manager.update_tls_config(update).await.unwrap();
    }

    {
        let manager = ConfigManager::new(temp_dir.path().to_path_buf()).unwrap();
        let config = manager.config().await;
        assert!(!config.tls.enable_interception);
    }
}

#[tokio::test]
async fn test_update_ui_rules_sort_mode_persists() {
    let temp_dir = TempDir::new().unwrap();

    {
        let manager = ConfigManager::new(temp_dir.path().to_path_buf()).unwrap();
        let updated = manager
            .update_ui_config(UiConfigUpdate {
                rules_sort_mode: Some("updated_desc".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(updated.rules_sort_mode, "updated_desc");
    }

    {
        let manager = ConfigManager::new(temp_dir.path().to_path_buf()).unwrap();
        let config = manager.config().await;
        assert_eq!(config.ui.rules_sort_mode, "updated_desc");
    }
}

#[tokio::test]
async fn test_values_operations() {
    let (_temp_dir, manager) = setup();

    manager.set_value("test_key", "test_value").await.unwrap();
    let value = manager.get_value("test_key").await;
    assert_eq!(value, Some("test_value".to_string()));

    manager.delete_value("test_key").await.unwrap();
    let value = manager.get_value("test_key").await;
    assert_eq!(value, None);
}

#[tokio::test]
async fn test_rules_operations() {
    let (_temp_dir, manager) = setup();

    let rule = RuleFile::new("test_rule", "example.com host://localhost");
    manager.save_rule(&rule).await.unwrap();

    let loaded = manager.load_rule("test_rule").await.unwrap();
    assert_eq!(loaded.name, "test_rule");
    assert_eq!(loaded.content, "example.com host://localhost");

    let rules = manager.list_rules().await.unwrap();
    assert!(rules.contains(&"test_rule".to_string()));

    manager.delete_rule("test_rule").await.unwrap();
    let rules = manager.list_rules().await.unwrap();
    assert!(!rules.contains(&"test_rule".to_string()));
}

#[tokio::test]
async fn test_rule_groups() {
    let (_temp_dir, manager) = setup();

    manager.enable_rule_group("group1").await.unwrap();
    assert!(manager.is_rule_group_enabled("group1").await);

    manager.disable_rule_group("group1").await.unwrap();
    assert!(!manager.is_rule_group_enabled("group1").await);
}

#[tokio::test]
async fn test_change_notification() {
    let (_temp_dir, manager) = setup();
    let mut receiver = manager.subscribe();

    let update = TlsConfigUpdate {
        enable_interception: Some(false),
        ..Default::default()
    };
    manager.update_tls_config(update).await.unwrap();

    let event = receiver.try_recv().unwrap();
    assert!(matches!(event, ConfigChangeEvent::TlsConfigChanged(_)));
}

#[tokio::test]
async fn test_update_traffic_config_rejects_out_of_range_max_records() {
    let (_temp_dir, manager) = setup();

    let err = manager
        .update_traffic_config(TrafficConfigUpdate {
            max_records: Some(MIN_TRAFFIC_MAX_RECORDS - 1),
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert!(err
        .to_string()
        .contains("traffic.max_records must be between 1000 and"));

    let err = manager
        .update_traffic_config(TrafficConfigUpdate {
            max_records: Some(MAX_TRAFFIC_MAX_RECORDS + 1),
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert!(err
        .to_string()
        .contains("traffic.max_records must be between 1000 and"));
}

#[tokio::test]
async fn test_update_traffic_config_rejects_out_of_range_breakpoint_timeout() {
    let (_temp_dir, manager) = setup();

    let err = manager
        .update_traffic_config(TrafficConfigUpdate {
            breakpoint_timeout_ms: Some(MIN_BREAKPOINT_TIMEOUT_MS - 1),
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert!(err
        .to_string()
        .contains("traffic.breakpoint_timeout_ms must be between 5000 and 300000"));

    let err = manager
        .update_traffic_config(TrafficConfigUpdate {
            breakpoint_timeout_ms: Some(MAX_BREAKPOINT_TIMEOUT_MS + 1),
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert!(err
        .to_string()
        .contains("traffic.breakpoint_timeout_ms must be between 5000 and 300000"));
}

#[tokio::test]
async fn test_data_dir_accessor_and_try_config() {
    let (temp_dir, manager) = setup();
    assert_eq!(manager.data_dir(), temp_dir.path());
    let cfg = manager.try_config().expect("try_config should succeed");
    assert_eq!(cfg.server.timeout_secs, 30);
}

#[tokio::test]
async fn test_update_config_generic_closure() {
    let (_temp_dir, manager) = setup();
    manager
        .update_config(|c| {
            c.server.timeout_secs = 99;
        })
        .await
        .unwrap();
    assert_eq!(manager.config().await.server.timeout_secs, 99);
}

#[tokio::test]
async fn test_update_tls_config_all_fields() {
    let (_temp_dir, manager) = setup();
    let update = TlsConfigUpdate {
        enable_interception: Some(true),
        intercept_exclude: Some(vec!["a.com".to_string()]),
        intercept_include: Some(vec!["b.com".to_string()]),
        app_intercept_exclude: Some(vec!["app1".to_string()]),
        app_intercept_include: Some(vec!["app2".to_string()]),
        ip_intercept_exclude: Some(vec!["1.1.1.1".to_string()]),
        ip_intercept_include: Some(vec!["2.2.2.2".to_string()]),
        unsafe_ssl: Some(true),
        disconnect_on_change: Some(false),
    };
    let result = manager.update_tls_config(update).await.unwrap();
    assert!(result.enable_interception);
    assert_eq!(result.intercept_exclude, vec!["a.com".to_string()]);
    assert_eq!(result.intercept_include, vec!["b.com".to_string()]);
    assert_eq!(result.app_intercept_exclude, vec!["app1".to_string()]);
    assert_eq!(result.app_intercept_include, vec!["app2".to_string()]);
    assert_eq!(result.ip_intercept_exclude, vec!["1.1.1.1".to_string()]);
    assert_eq!(result.ip_intercept_include, vec!["2.2.2.2".to_string()]);
    assert!(result.unsafe_ssl);
    assert!(!result.disconnect_on_change);
}

#[tokio::test]
async fn test_update_access_config() {
    let (_temp_dir, manager) = setup();
    let mut receiver = manager.subscribe();
    manager
        .update_access_config(AccessConfigUpdate {
            mode: Some(bifrost_core::AccessMode::AllowAll),
            whitelist: Some(vec!["10.0.0.0/8".to_string()]),
            allow_lan: Some(true),
            userpass: Some(None),
        })
        .await
        .unwrap();
    let config = manager.config().await;
    assert_eq!(config.access.mode, bifrost_core::AccessMode::AllowAll);
    assert_eq!(config.access.whitelist, vec!["10.0.0.0/8".to_string()]);
    assert!(config.access.allow_lan);
    let event = receiver.try_recv().unwrap();
    assert!(matches!(event, ConfigChangeEvent::AccessConfigChanged));
}

#[tokio::test]
async fn test_update_access_config_write_failure_preserves_in_memory_config() {
    let (temp_dir, manager) = setup();
    let mut receiver = manager.subscribe();
    let original = manager.config().await.access;
    let config_path = temp_dir.path().join("config.toml");
    std::fs::remove_file(&config_path).unwrap();
    std::fs::create_dir(&config_path).unwrap();

    let result = manager
        .update_access_config(AccessConfigUpdate {
            mode: Some(bifrost_core::AccessMode::AllowAll),
            whitelist: Some(vec!["10.0.0.0/8".to_string()]),
            allow_lan: Some(true),
            userpass: Some(Some(bifrost_core::UserPassAuthConfig {
                enabled: true,
                accounts: vec![bifrost_core::UserPassAccountConfig {
                    username: "alice".to_string(),
                    password: Some("secret".to_string()),
                    enabled: true,
                }],
                loopback_requires_auth: true,
            })),
        })
        .await;

    assert!(result.is_err());
    let current = manager.config().await.access;
    assert_eq!(current.mode, original.mode);
    assert_eq!(current.whitelist, original.whitelist);
    assert_eq!(current.allow_lan, original.allow_lan);
    assert!(current.userpass.is_none());
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn test_userpass_passwords_are_encrypted_at_rest_and_decrypted_on_load() {
    let temp_dir = TempDir::new().unwrap();
    let password = "bifrost-local-secret:not-json";

    {
        let manager = ConfigManager::new(temp_dir.path().to_path_buf()).unwrap();
        manager
            .update_access_config(AccessConfigUpdate {
                userpass: Some(Some(bifrost_core::UserPassAuthConfig {
                    enabled: true,
                    accounts: vec![bifrost_core::UserPassAccountConfig {
                        username: "alice".to_string(),
                        password: Some(password.to_string()),
                        enabled: true,
                    }],
                    loopback_requires_auth: true,
                })),
                ..Default::default()
            })
            .await
            .unwrap();

        let config = manager.config().await;
        let stored_password = config.access.userpass.unwrap().accounts[0]
            .password
            .as_deref()
            .unwrap()
            .to_string();
        assert_eq!(stored_password, password);
    }

    let raw_config = std::fs::read_to_string(temp_dir.path().join("config.toml")).unwrap();
    assert!(!raw_config.contains(password));
    assert!(raw_config.contains("bifrost-local-secret:"));
    assert!(temp_dir.path().join("local_config_secret.key").is_file());

    {
        let manager = ConfigManager::new(temp_dir.path().to_path_buf()).unwrap();
        let config = manager.config().await;
        let userpass = config.access.userpass.expect("userpass config");
        assert!(userpass.enabled);
        assert!(userpass.loopback_requires_auth);
        assert_eq!(userpass.accounts[0].username, "alice");
        assert_eq!(userpass.accounts[0].password.as_deref(), Some(password));
    }
}

#[tokio::test]
async fn test_update_system_proxy_config() {
    let (_temp_dir, manager) = setup();
    let mut receiver = manager.subscribe();
    manager
        .update_system_proxy_config(SystemProxyConfigUpdate {
            enabled: Some(true),
            bypass: Some("localhost".to_string()),
            auto_enable: Some(true),
            recovery_mode: Some(crate::SystemProxyRecoveryMode::FailClosed),
            recovery_grace_secs: Some(4),
        })
        .await
        .unwrap();
    let config = manager.config().await;
    assert!(config.system_proxy.enabled);
    assert_eq!(config.system_proxy.bypass, "localhost");
    assert!(config.system_proxy.auto_enable);
    assert_eq!(
        config.system_proxy.recovery_mode,
        crate::SystemProxyRecoveryMode::FailClosed
    );
    assert_eq!(config.system_proxy.recovery_grace_secs, 4);
    let event = receiver.try_recv().unwrap();
    assert!(matches!(event, ConfigChangeEvent::SystemProxyConfigChanged));
}

#[tokio::test]
async fn test_update_server_config() {
    let (_temp_dir, manager) = setup();
    let result = manager
        .update_server_config(ServerConfigUpdate {
            timeout_secs: Some(60),
            http1_max_header_size: Some(1024),
            http2_max_header_list_size: Some(2048),
            websocket_handshake_max_header_size: Some(4096),
        })
        .await
        .unwrap();
    assert_eq!(result.timeout_secs, 60);
    assert_eq!(result.http1_max_header_size, 1024);
    assert_eq!(result.http2_max_header_list_size, 2048);
    assert_eq!(result.websocket_handshake_max_header_size, 4096);
}

#[tokio::test]
async fn test_update_traffic_config_accepts_valid_fields() {
    let (_temp_dir, manager) = setup();
    let result = manager
        .update_traffic_config(TrafficConfigUpdate {
            max_records: Some(MIN_TRAFFIC_MAX_RECORDS + 1),
            max_body_memory_size: Some(1234),
            max_body_buffer_size: Some(5678),
            max_body_probe_size: Some(910),
            super_performance_mode: Some(true),
            binary_traffic_performance_mode: Some(false),
            inject_bifrost_badge: Some(false),
            file_retention_days: Some(15),
            sse_stream_flush_bytes: Some(111),
            sse_stream_flush_interval_ms: Some(222),
            ws_payload_flush_bytes: Some(333),
            ws_payload_flush_interval_ms: Some(444),
            ws_payload_max_open_files: Some(55),
            breakpoint_timeout_ms: Some(MIN_BREAKPOINT_TIMEOUT_MS + 1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(result.max_records, MIN_TRAFFIC_MAX_RECORDS + 1);
    assert_eq!(result.max_body_memory_size, 1234);
    assert_eq!(result.max_body_buffer_size, 5678);
    assert_eq!(result.max_body_probe_size, 910);
    assert!(result.super_performance_mode);
    assert!(!result.binary_traffic_performance_mode);
    assert!(!result.inject_bifrost_badge);
    assert_eq!(result.file_retention_days, 15);
    assert_eq!(result.ws_payload_max_open_files, 55);
}

#[tokio::test]
async fn test_update_traffic_config_rejects_out_of_range_db_size() {
    let (_temp_dir, manager) = setup();
    let err = manager
        .update_traffic_config(TrafficConfigUpdate {
            max_db_size_bytes: Some(MIN_TRAFFIC_MAX_DB_SIZE_BYTES - 1),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("max_db_size_bytes"));

    let ok = manager
        .update_traffic_config(TrafficConfigUpdate {
            max_db_size_bytes: Some(MIN_TRAFFIC_MAX_DB_SIZE_BYTES),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(ok.max_db_size_bytes, MIN_TRAFFIC_MAX_DB_SIZE_BYTES);
}

#[tokio::test]
async fn test_update_sandbox_config() {
    use crate::unified_config::{
        SandboxFileConfigUpdate, SandboxLimitsConfigUpdate, SandboxNetConfigUpdate,
    };
    let (_temp_dir, manager) = setup();
    let result = manager
        .update_sandbox_config(SandboxConfigUpdate {
            file: Some(SandboxFileConfigUpdate {
                sandbox_dir: Some("/tmp/sb".to_string()),
                allowed_dirs: Some(vec!["/tmp".to_string()]),
                max_bytes: Some(1000),
            }),
            net: Some(SandboxNetConfigUpdate {
                enabled: Some(true),
                allow_private_network: Some(true),
                timeout_ms: Some(2000),
                max_request_bytes: Some(3000),
                max_response_bytes: Some(4000),
            }),
            limits: Some(SandboxLimitsConfigUpdate {
                timeout_ms: Some(5000),
                max_memory_bytes: Some(6000),
                max_decode_input_bytes: Some(7000),
                max_decompress_output_bytes: Some(8000),
            }),
        })
        .await
        .unwrap();
    assert_eq!(result.file.sandbox_dir, "/tmp/sb");
    assert_eq!(result.file.max_bytes, 1000);
    assert!(result.net.enabled);
    assert!(result.net.allow_private_network);
    assert_eq!(result.net.timeout_ms, 2000);
    assert_eq!(result.limits.timeout_ms, 5000);
    assert_eq!(result.limits.max_memory_bytes, 6000);
}

#[tokio::test]
async fn test_get_and_update_ui_config() {
    let (_temp_dir, manager) = setup();
    let initial = manager.get_ui_config().await;
    let _ = initial.rules_sort_mode;
    let updated = manager
        .update_ui_config(UiConfigUpdate {
            detail_panel_collapsed: Some(true),
            rules_sort_mode: Some("custom".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(updated.detail_panel_collapsed);
    assert_eq!(updated.rules_sort_mode, "custom");
}

#[tokio::test]
async fn test_update_sync_config() {
    let (_temp_dir, manager) = setup();
    let mut receiver = manager.subscribe();
    let result = manager
        .update_sync_config(SyncConfigUpdate {
            enabled: Some(true),
            auto_sync: Some(true),
            remote_base_url: Some("https://sync.example".to_string()),
            probe_interval_secs: Some(30),
            connect_timeout_ms: Some(5000),
        })
        .await
        .unwrap();
    assert!(result.enabled);
    assert!(result.auto_sync);
    assert_eq!(result.remote_base_url, "https://sync.example");
    assert_eq!(result.probe_interval_secs, 30);
    assert_eq!(result.connect_timeout_ms, 5000);
    let event = receiver.try_recv().unwrap();
    assert!(matches!(event, ConfigChangeEvent::SyncConfigChanged));
}

#[tokio::test]
async fn test_rule_ordering_and_enable_disable() {
    let (_temp_dir, manager) = setup();
    manager
        .save_rule(&RuleFile::new("r1", "a.com host://x"))
        .await
        .unwrap();
    manager
        .save_rule(&RuleFile::new("r2", "b.com host://y"))
        .await
        .unwrap();

    manager
        .reorder_rules(&["r2".to_string(), "r1".to_string()])
        .await
        .unwrap();

    manager.set_rule_enabled("r1", false).await.unwrap();
    let enabled = manager.load_enabled_rules().await.unwrap();
    assert!(!enabled.iter().any(|r| r.name == "r1"));

    let all = manager.load_all_rules().await.unwrap();
    assert_eq!(all.len(), 2);

    let storage = manager.rules_storage().await;
    assert!(storage.list().unwrap().contains(&"r2".to_string()));
}

#[tokio::test]
async fn test_values_list_and_hashmap() {
    let (_temp_dir, manager) = setup();
    manager.set_value("k1", "v1").await.unwrap();
    manager.set_value("k2", "v2").await.unwrap();

    let mut list = manager.list_values().await;
    list.sort();
    assert_eq!(
        list,
        vec![
            ("k1".to_string(), "v1".to_string()),
            ("k2".to_string(), "v2".to_string())
        ]
    );

    let map = manager.values_as_hashmap().await;
    assert_eq!(map.get("k1"), Some(&"v1".to_string()));

    let storage = manager.values_storage().await;
    assert_eq!(storage.get_value("k2"), Some("v2".to_string()));
}

#[tokio::test]
async fn test_enabled_rule_groups_listing() {
    let (_temp_dir, manager) = setup();
    manager.enable_rule_group("g1").await.unwrap();
    manager.enable_rule_group("g2").await.unwrap();
    let groups = manager.enabled_rule_groups().await;
    assert!(groups.contains(&"g1".to_string()));
    assert!(groups.contains(&"g2".to_string()));
}

#[tokio::test]
async fn test_userpass_last_connected_at_roundtrip() {
    let (_temp_dir, manager) = setup();
    manager
        .record_userpass_last_connected_at("alice", 12345)
        .await
        .unwrap();
    let map = manager.userpass_last_connected_at().await;
    assert_eq!(map.get("alice"), Some(&12345));

    let mut replacement = std::collections::HashMap::new();
    replacement.insert("bob".to_string(), 999u64);
    manager
        .replace_userpass_last_connected_at(replacement)
        .await
        .unwrap();
    let map = manager.userpass_last_connected_at().await;
    assert_eq!(map.get("bob"), Some(&999));
    assert_eq!(map.get("alice"), None);
}

#[tokio::test]
async fn test_notify_sends_event() {
    let (_temp_dir, manager) = setup();
    let mut receiver = manager.subscribe();
    manager.notify(ConfigChangeEvent::ScriptsChanged).unwrap();
    let event = receiver.try_recv().unwrap();
    assert!(matches!(event, ConfigChangeEvent::ScriptsChanged));
}

#[test]
fn test_rules_change_origin_sync_wake_policy_matches_event_source() {
    assert!(RulesChangeOrigin::LocalApi.should_wake_sync());
    assert!(RulesChangeOrigin::Filesystem.should_wake_sync());
    assert!(RulesChangeOrigin::Unknown.should_wake_sync());
    assert!(!RulesChangeOrigin::RemoteSync.should_wake_sync());
}

#[tokio::test]
async fn test_clone_preserves_config() {
    let (_temp_dir, manager) = setup();
    manager.set_value("k", "v").await.unwrap();
    let cloned = manager.clone();
    assert_eq!(cloned.data_dir(), manager.data_dir());
    assert_eq!(cloned.get_value("k").await, Some("v".to_string()));
}

#[test]
fn test_migrate_from_legacy_maps_fields() {
    // Drive the private migration helper directly so the mapping is exercised
    // deterministically (independent of serde untagged-parse ordering).
    let legacy = LegacyBifrostConfig {
        enable_tls_interception: false,
        intercept_exclude: vec!["skip.com".to_string()],
        intercept_include: vec!["keep.com".to_string()],
        disconnect_on_config_change: false,
        ..Default::default()
    };
    let data_dir = Path::new("/tmp/bifrost-migrate-test");
    let migrated = ConfigManager::migrate_from_legacy(&legacy, data_dir);

    assert!(!migrated.tls.enable_interception);
    assert_eq!(migrated.tls.intercept_exclude, vec!["skip.com".to_string()]);
    assert_eq!(migrated.tls.intercept_include, vec!["keep.com".to_string()]);
    assert!(!migrated.tls.disconnect_on_change);
    // max_records clamped into the valid range.
    assert!(migrated.traffic.max_records >= MIN_TRAFFIC_MAX_RECORDS);
    assert!(migrated.traffic.max_records <= MAX_TRAFFIC_MAX_RECORDS);
}

#[tokio::test]
async fn test_invalid_config_returns_error() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::create_dir_all(temp_dir.path()).unwrap();
    // Garbage that parses as neither UnifiedConfig nor LegacyBifrostConfig.
    std::fs::write(
        temp_dir.path().join("config.toml"),
        "this is = = not valid toml at = all [[[",
    )
    .unwrap();
    let result = ConfigManager::new(temp_dir.path().to_path_buf());
    assert!(result.is_err());
}

#[tokio::test]
async fn system_proxy_intent_revision_advances_for_repeated_explicit_requests() {
    let (temp, manager) = setup();
    for revision in 1..=2 {
        manager
            .update_system_proxy_config(SystemProxyConfigUpdate {
                enabled: Some(true),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            manager.config().await.system_proxy.intent_revision,
            revision
        );
    }
    let reloaded = ConfigManager::new(temp.path().to_path_buf()).unwrap();
    assert_eq!(reloaded.config().await.system_proxy.intent_revision, 2);
}

#[tokio::test]
async fn failed_system_proxy_save_keeps_previous_config_and_does_not_notify() {
    let (temp, manager) = setup();
    let mut receiver = manager.subscribe();
    let before = manager.config().await.system_proxy;
    std::fs::remove_file(temp.path().join("config.toml")).unwrap();
    std::fs::create_dir(temp.path().join("config.toml")).unwrap();
    assert!(manager
        .update_system_proxy_config(SystemProxyConfigUpdate {
            enabled: Some(!before.enabled),
            bypass: Some("changed.example".to_string()),
            ..Default::default()
        })
        .await
        .is_err());
    let after = manager.config().await.system_proxy;
    assert_eq!(after.enabled, before.enabled);
    assert_eq!(after.bypass, before.bypass);
    assert_eq!(after.intent_revision, before.intent_revision);
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn newer_cross_process_proxy_intent_survives_unrelated_stale_config_save() {
    let (temp, manager) = setup();
    let stale = ConfigManager::new(temp.path().to_path_buf()).unwrap();
    manager
        .update_system_proxy_config(SystemProxyConfigUpdate {
            enabled: Some(false),
            bypass: Some("new.example".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    stale
        .update_config(|config| config.server.timeout_secs = 47)
        .await
        .unwrap();
    let disk = crate::read_persisted_system_proxy_config(temp.path()).unwrap();
    assert!(!disk.enabled);
    assert_eq!(disk.intent_revision, 1);
    assert_eq!(disk.bypass, "new.example");
    // A later explicit request through the stale manager builds on disk state.
    stale
        .update_system_proxy_config(SystemProxyConfigUpdate {
            enabled: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    let disk = crate::read_persisted_system_proxy_config(temp.path()).unwrap();
    assert!(disk.enabled);
    assert_eq!(disk.intent_revision, 2);
    assert_eq!(disk.bypass, "new.example");
}

#[test]
fn concurrent_config_managers_serialize_durable_proxy_revisions() {
    let (temp, _) = setup();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = [true, false]
        .into_iter()
        .map(|enabled| {
            let manager = ConfigManager::new(temp.path().to_path_buf()).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                futures::executor::block_on(manager.update_system_proxy_config(
                    SystemProxyConfigUpdate {
                        enabled: Some(enabled),
                        ..Default::default()
                    },
                ))
                .unwrap();
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let disk = crate::read_persisted_system_proxy_config(temp.path()).unwrap();
    assert_eq!(disk.intent_revision, 2);
}

#[test]
fn concurrent_writers_return_their_own_accepted_proxy_snapshot() {
    let (temp, _) = setup();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = [true, false]
        .into_iter()
        .map(|enabled| {
            let manager = ConfigManager::new(temp.path().to_path_buf()).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let snapshot = futures::executor::block_on(
                    manager.update_system_proxy_config_with_snapshot(SystemProxyConfigUpdate {
                        enabled: Some(enabled),
                        bypass: Some(format!("request-{enabled}.example")),
                        ..Default::default()
                    }),
                )
                .unwrap();
                assert_eq!(snapshot.enabled, enabled);
                assert_eq!(snapshot.bypass, format!("request-{enabled}.example"));
                snapshot
            })
        })
        .collect();
    let mut snapshots: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    snapshots.sort_by_key(|snapshot| snapshot.intent_revision);
    assert_eq!(snapshots[0].intent_revision, 1);
    assert_eq!(snapshots[1].intent_revision, 2);
    let persisted = crate::read_persisted_system_proxy_config(temp.path()).unwrap();
    assert_eq!(persisted.intent_revision, snapshots[1].intent_revision);
    assert_eq!(persisted.enabled, snapshots[1].enabled);
    assert_ne!(snapshots[0].enabled, persisted.enabled);
    // The first returned snapshot is not relabeled with the later disk revision.
    assert_eq!(snapshots[0].intent_revision, 1);
}
