    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    fn spawn_test_http_server(
        responses: Vec<(&'static str, &'static str)>,
    ) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_for_thread = Arc::clone(&seen);
        let handle = thread::spawn(move || {
            for (status_line, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 2048];
                let n = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..n]);
                if let Some(first_line) = request.lines().next() {
                    seen_for_thread.lock().unwrap().push(first_line.to_string());
                }
                let response = format!(
                    "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://{addr}/_bifrost/"), seen, handle)
    }

    fn sample_runtime() -> RuntimeInfo {
        RuntimeInfo {
            pid: 1234,
            port: 8800,
            socks5_port: Some(1080),
            host: Some("127.0.0.1".to_string()),
            started_at_ms: None,
            binary_path: None,
        }
    }

    #[cfg(target_os = "macos")]
    fn find_menu_item<'a>(entries: &'a [MenuEntry], id: &str) -> Option<&'a MenuItemDef> {
        entries.iter().find_map(|entry| match entry {
            MenuEntry::Item(item) if item.id == id => Some(item),
            MenuEntry::Submenu(submenu) => find_menu_item(&submenu.children, id),
            _ => None,
        })
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_menu_bar_stats_title_uses_running_system_stats_only() {
        let snapshot = MenuDataSnapshot {
            runtime: Some(sample_runtime()),
            custom_config: None,
            rules: Vec::new(),
            recent_rule_targets: Vec::new(),
            system_proxy: None,
            bin_available: true,
            update_available: None,
            system_stats: Some(SystemStatsMenuLines {
                system: "System: CPU 23% | Memory 18.0 GB / 32.0 GB | Disk 59%".to_string(),
                network: "Network: Up 1.5 MB/s | Down 512 KB/s".to_string(),
                menu_bar: "C20% | M55% | D55% | ↑1.5 M/s ↓512 K/s".to_string(),
            }),
        };

        assert_eq!(
            menu_bar_stats_title(&snapshot, ServiceState::Running),
            Some("C20% | M55% | D55% | ↑1.5 M/s ↓512 K/s".to_string())
        );
        assert_eq!(menu_bar_stats_title(&snapshot, ServiceState::Stopped), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_menu_bar_stats_bitmap_is_compact_and_non_empty() {
        let bitmap =
            render_menu_bar_stats_bitmap("C20% | M55% | D55% | ↑1.5 M/s ↓512 K/s")
                .expect("bitmap");

        assert_eq!(bitmap.height, 36);
        assert!(bitmap.width <= 1400);
        assert!(bitmap.rgba.chunks_exact(4).any(|pixel| pixel[3] == 255));

        let icon_region_max_alpha = bitmap
            .rgba
            .chunks_exact(4)
            .enumerate()
            .filter_map(|(idx, pixel)| {
                let x = idx as u32 % bitmap.width;
                let y = idx as u32 / bitmap.width;
                (x < 32 && y < bitmap.height).then_some(pixel[3])
            })
            .max()
            .unwrap_or(0);
        assert_eq!(icon_region_max_alpha, 255);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_menu_bar_stats_columns_align_value_and_label_rows() {
        let font = menu_bar_stats_font().expect("font");
        let rows = menu_bar_stats_rows("C20% | M55% | D55% | ↑1.5 M/s ↓512 K/s");
        let columns = menu_bar_stats_columns(font, &rows, 28.0, 9.5);

        assert_eq!(
            rows.values,
            vec!["C20%", "M55%", "D55%", "↑1.5 M/s ↓512 K/s"]
        );
        assert!(rows.labels.is_empty());
        assert_eq!(columns.len(), 4);
        assert!(columns.windows(2).all(|pair| pair[0].x < pair[1].x));
        assert_eq!(columns[0].value_font_px, 28.0);
        assert_eq!(columns[0].label_font_px, 9.5);
        assert_eq!(columns[3].value_font_px, 28.0);
        assert_eq!(columns[3].label_font_px, 28.0);
        assert!(columns.windows(2).all(|pair| {
            pair[1].x - (pair[0].x + pair[0].width)
                == MENU_BAR_STATS_SEPARATOR_GAP * 2 + MENU_BAR_STATS_SEPARATOR_WIDTH
        }));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_native_stats_view_is_default_on_with_explicit_opt_out() {
        assert!(native_stats_view_enabled_from_env(None));
        assert!(native_stats_view_enabled_from_env(Some("1")));
        assert!(native_stats_view_enabled_from_env(Some("true")));
        assert!(native_stats_view_enabled_from_env(Some("yes")));

        assert!(!native_stats_view_enabled_from_env(Some("0")));
        assert!(!native_stats_view_enabled_from_env(Some("false")));
        assert!(!native_stats_view_enabled_from_env(Some("no")));
        assert!(!native_stats_view_enabled_from_env(Some("off")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_native_menu_bar_stats_rows_convert_single_row_to_reference_layout() {
        let rows = native_menu_bar_stats_rows("C10% | M75% | D65% | ↑11.7 K/s ↓25.2 K/s");

        assert_eq!(
            rows.values,
            vec!["10%", "75%", "65%", "↑11.7 K/s"]
        );
        assert_eq!(
            rows.labels,
            vec!["CPU", "MEM", "SSD", "↓25.2 K/s"]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_native_menu_bar_stats_rows_follow_each_tray_switch() {
        let snapshot = system_stats::SystemStatsSnapshot {
            cpu_percent: 10.0,
            memory_used_bytes: 16 * 1024 * 1024 * 1024,
            memory_total_bytes: 32 * 1024 * 1024 * 1024,
            disk_used_percent: Some(65.0),
            network_down_bytes_per_sec: Some(2048),
            network_up_bytes_per_sec: Some(1024),
        };

        let cases = [
            (
                bifrost_storage::TraySystemStatsItems {
                    cpu: false,
                    memory: true,
                    disk: true,
                    upload: true,
                    download: true,
                },
                vec!["50%", "65%", "↑1.0 K/s"],
                vec!["MEM", "SSD", "↓2.0 K/s"],
            ),
            (
                bifrost_storage::TraySystemStatsItems {
                    cpu: true,
                    memory: false,
                    disk: true,
                    upload: true,
                    download: true,
                },
                vec!["10%", "65%", "↑1.0 K/s"],
                vec!["CPU", "SSD", "↓2.0 K/s"],
            ),
            (
                bifrost_storage::TraySystemStatsItems {
                    cpu: true,
                    memory: true,
                    disk: false,
                    upload: true,
                    download: true,
                },
                vec!["10%", "50%", "↑1.0 K/s"],
                vec!["CPU", "MEM", "↓2.0 K/s"],
            ),
            (
                bifrost_storage::TraySystemStatsItems {
                    cpu: true,
                    memory: true,
                    disk: true,
                    upload: false,
                    download: true,
                },
                vec!["10%", "50%", "65%", ""],
                vec!["CPU", "MEM", "SSD", "↓2.0 K/s"],
            ),
            (
                bifrost_storage::TraySystemStatsItems {
                    cpu: true,
                    memory: true,
                    disk: true,
                    upload: true,
                    download: false,
                },
                vec!["10%", "50%", "65%", "↑1.0 K/s"],
                vec!["CPU", "MEM", "SSD", ""],
            ),
        ];

        for (items, expected_values, expected_labels) in cases {
            let lines = system_stats::menu_lines(&snapshot, &items);
            let rows = native_menu_bar_stats_rows(&lines.menu_bar);

            assert_eq!(rows.values, expected_values);
            assert_eq!(rows.labels, expected_labels);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_native_network_column_uses_same_font_for_up_and_down_rows() {
        let font = menu_bar_stats_font().expect("font");
        let rows = native_menu_bar_stats_rows("C10% | M75% | D65% | ↑11.7 K/s ↓25.2 K/s");
        let columns = menu_bar_stats_columns(font, &rows, 24.0, 14.0);

        let network = columns.last().expect("network column");
        assert_eq!(network.value_font_px, 24.0);
        assert_eq!(network.label_font_px, 24.0);
        assert!(network.width >= measure_text_width(font, "↓999.9 M/s", 24.0));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_native_network_column_uses_fixed_slot_with_graphic_arrow() {
        let font = menu_bar_stats_font().expect("font");
        let actual_width = measure_menu_bar_stats_part_width(font, "↑11.7 K/s", 24.0);
        let text_width = measure_text_width(font, "11.7 K/s", 24.0);
        let stable_width = menu_bar_network_stable_width(font, 24.0);

        assert_eq!(
            actual_width,
            MENU_BAR_NETWORK_ARROW_WIDTH + MENU_BAR_NETWORK_ARROW_GAP + text_width
        );
        assert!(stable_width > actual_width);
        assert_eq!(menu_bar_network_text_without_arrow("↓25.2 K/s"), "25.2 K/s");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_native_stats_accessibility_label_reflects_status_item_content() {
        assert_eq!(
            native_stats_accessibility_label(Some("C10% | M50% | ↑1.0 K/s ↓2.0 K/s")),
            "Bifrost: C10% | M50% | ↑1.0 K/s ↓2.0 K/s"
        );
        assert_eq!(native_stats_accessibility_label(None), "Bifrost");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_native_menu_bar_stats_bitmap_uses_full_height_and_continuous_separators() {
        let bitmap = render_native_menu_bar_stats_bitmap(
            "C10% | M75% | D65% | ↑11.7 K/s ↓25.2 K/s",
        )
        .expect("native bitmap");

        assert_eq!(bitmap.height, 48);
        assert!(bitmap.width > 200);
        assert!(bitmap.rgba.chunks_exact(4).any(|pixel| pixel[3] == 255));

        let separator_x = (0..bitmap.width)
            .find(|x| {
                (5..=42).all(|y| {
                    let idx = ((y * bitmap.width + *x) * 4) as usize;
                    bitmap.rgba.get(idx + 3).copied().unwrap_or_default() >= 200
                })
            })
            .expect("continuous separator");
        assert!(separator_x > 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_menu_hides_system_stats_rows() {
        let snapshot = MenuDataSnapshot {
            runtime: Some(sample_runtime()),
            custom_config: None,
            rules: Vec::new(),
            recent_rule_targets: Vec::new(),
            system_proxy: None,
            bin_available: true,
            update_available: None,
            system_stats: Some(SystemStatsMenuLines {
                system: "System: CPU 23% | Memory 18.0 GB / 32.0 GB | Disk 59%".to_string(),
                network: "Network: Up 1.5 MB/s | Down 512 KB/s".to_string(),
                menu_bar: "C20% | M55% | D55% | ↑1.5 M/s ↓512 K/s".to_string(),
            }),
        };

        let menu = build_menu_from_snapshot(
            &snapshot,
            ServiceState::Running,
            None,
            false,
            "/tmp/.bifrost",
            false,
        );

        assert!(find_menu_item(&menu, "_system_stats").is_none());
        assert!(find_menu_item(&menu, "_network_stats").is_none());
    }

    #[test]
    fn test_missing_runtime_file_uses_parent_process_fallback_for_menu() {
        let data_dir = std::env::temp_dir().join(format!(
            "bifrost-tray-runtime-fallback-{}",
            std::process::id()
        ));
        let args = TrayArgs {
            data_dir: data_dir.clone(),
            runtime_file: data_dir.join("missing-runtime.json"),
            parent_pid: std::process::id(),
            admin_url: Some("http://127.0.0.1:9900/_bifrost/".to_string()),
            port: Some(9900),
            bifrost_bin: Some(PathBuf::from("/tmp/bifrost")),
            start_args: Vec::new(),
        };

        let runtime = runtime_for_menu(&args).expect("parent process fallback runtime");
        assert_eq!(runtime.pid, std::process::id());
        assert_eq!(runtime.admin_url(), "http://127.0.0.1:9900/_bifrost/");

        let state = determine_state(Some(&runtime), args.parent_pid);
        assert_eq!(state, ServiceState::Running);
        let menu = menu::build_menu(
            Some(&runtime),
            state,
            None,
            false,
            None,
            data_dir.to_string_lossy().as_ref(),
            true,
            &[],
            &[],
            None,
            None,
            false,
            None,
        );
        let status = match &menu[0] {
            menu::MenuEntry::Item(item) => item.label.as_str(),
            menu::MenuEntry::Submenu(_) => panic!("expected status item"),
        };
        assert_eq!(status, "Bifrost: Running on 127.0.0.1:9900");
    }

    #[test]
    fn test_pure_tray_icon_event_does_not_refresh_native_menu() {
        assert!(!should_refresh_native_menu(
            false, false, false, false, false
        ));
    }

    #[test]
    fn test_background_changes_request_native_menu_refresh() {
        assert!(should_refresh_native_menu(true, false, false, false, false));
        assert!(should_refresh_native_menu(false, true, false, false, false));
        assert!(should_refresh_native_menu(false, false, false, false, true));
    }

    #[test]
    fn test_service_idle_auto_exit_after_timeout_without_restart() {
        let start = Instant::now();
        let mut idle_since = None;

        assert!(!should_auto_exit_for_service_idle(
            &mut idle_since,
            STATE_STOPPED,
            OP_IDLE,
            start
        ));
        assert_eq!(idle_since, Some(start));
        assert!(!should_auto_exit_for_service_idle(
            &mut idle_since,
            STATE_STOPPED,
            OP_IDLE,
            start + SERVICE_IDLE_EXIT_TIMEOUT - Duration::from_secs(1)
        ));
        assert!(should_auto_exit_for_service_idle(
            &mut idle_since,
            STATE_STOPPED,
            OP_IDLE,
            start + SERVICE_IDLE_EXIT_TIMEOUT
        ));
    }

    #[test]
    fn test_service_idle_auto_exit_resets_after_running_or_starting() {
        let start = Instant::now();
        let mut idle_since = Some(start);

        assert!(!should_auto_exit_for_service_idle(
            &mut idle_since,
            STATE_RUNNING,
            OP_IDLE,
            start + Duration::from_secs(20)
        ));
        assert_eq!(idle_since, None);

        assert!(!should_auto_exit_for_service_idle(
            &mut idle_since,
            STATE_STOPPED,
            OP_STARTING,
            start + SERVICE_IDLE_EXIT_TIMEOUT + Duration::from_secs(1)
        ));
        assert_eq!(idle_since, None);

        let after_failed_start = start + SERVICE_IDLE_EXIT_TIMEOUT + Duration::from_secs(2);
        assert!(!should_auto_exit_for_service_idle(
            &mut idle_since,
            STATE_STOPPED,
            OP_START_FAILED,
            after_failed_start
        ));
        assert_eq!(idle_since, Some(after_failed_start));
    }

    #[test]
    fn test_tray_start_service_uses_detached_daemon() {
        let extra = vec![
            "--unsafe-ssl".to_string(),
            "--skip-cert-check".to_string(),
        ];

        let args = build_service_start_args(Some(18897), &extra);

        assert_eq!(
            args,
            vec![
                "start",
                "--daemon",
                "--no-tray",
                "--no-system-proxy",
                "-p",
                "18897",
                "--unsafe-ssl",
                "--skip-cert-check",
            ]
        );
    }

    #[test]
    fn test_recent_tray_interaction_defers_structural_replacement() {
        assert!(!should_replace_native_menu(false, false, true));
        assert!(should_replace_native_menu(false, false, false));
    }

    #[test]
    fn test_upgrade_operation_status_and_busy() {
        assert_eq!(
            operation_status_label(OP_UPGRADING),
            Some("Bifrost: Updating…")
        );
        assert_eq!(
            operation_status_label(OP_UPGRADE_FAILED),
            Some("Bifrost: Update failed - open logs")
        );
        assert!(operation_busy(OP_UPGRADING));
        assert!(!operation_busy(OP_UPGRADE_FAILED));
    }

    #[test]
    fn test_upgrade_status_label_includes_percent_only_while_downloading() {
        assert_eq!(
            upgrade_status_label(OP_UPGRADING, 42),
            Some("Bifrost: Updating… 42%".to_string())
        );
        // No percent available -> fall back to static label.
        assert_eq!(upgrade_status_label(OP_UPGRADING, UPGRADE_PERCENT_NONE), None);
        // Not an upgrade -> no dynamic label.
        assert_eq!(upgrade_status_label(OP_STARTING, 42), None);
    }

    #[test]
    fn test_tray_update_cache_missing_or_stale_requires_fetch() {
        let dir = tempfile::tempdir().unwrap();

        assert!(should_fetch_update_cache(dir.path()));

        let fresh_cache = bifrost_core::version_check::VersionCache {
            latest_version: "999.0.0".to_string(),
            release_highlights: vec!["fresh".to_string()],
            checked_at: chrono::Utc::now(),
        };
        assert!(write_version_cache(dir.path(), &fresh_cache));
        assert!(!should_fetch_update_cache(dir.path()));

        let stale_cache = bifrost_core::version_check::VersionCache {
            checked_at: chrono::Utc::now()
                - chrono::Duration::seconds(TRAY_UPDATE_CACHE_MAX_AGE_SECS + 1),
            ..fresh_cache
        };
        assert!(write_version_cache(dir.path(), &stale_cache));
        assert!(should_fetch_update_cache(dir.path()));
    }

    #[test]
    fn test_detect_update_available_uses_tray_cache_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let cache = bifrost_core::version_check::VersionCache {
            latest_version: "999.0.0".to_string(),
            release_highlights: vec!["background check".to_string()],
            checked_at: chrono::Utc::now(),
        };
        assert!(write_version_cache(dir.path(), &cache));

        assert_eq!(
            detect_update_available(dir.path()),
            Some("999.0.0".to_string())
        );

        let old_cache = bifrost_core::version_check::VersionCache {
            latest_version: "0.0.0".to_string(),
            release_highlights: Vec::new(),
            checked_at: chrono::Utc::now(),
        };
        assert!(write_version_cache(dir.path(), &old_cache));
        assert_eq!(detect_update_available(dir.path()), None);
    }

    #[test]
    fn test_explicit_reload_and_user_action_replace_native_menu() {
        assert!(should_replace_native_menu(true, false, true));
        assert!(should_replace_native_menu(false, true, true));
    }

    #[test]
    fn test_same_shape_native_menu_can_refresh_items_in_place() {
        let rt = sample_runtime();
        let initial = menu::build_menu(
            Some(&rt),
            ServiceState::Running,
            None,
            false,
            None,
            "/tmp/.bifrost",
            true,
            &[],
            &[],
            None,
            None,
            false,
            None,
        );

        let busy = menu::build_menu(
            Some(&rt),
            ServiceState::Running,
            Some("Bifrost: Starting..."),
            true,
            None,
            "/tmp/.bifrost",
            true,
            &[],
            &[],
            None,
            None,
            false,
            None,
        );

        assert_eq!(menu_shape(&initial), menu_shape(&busy));
    }

    #[test]
    fn test_changed_shape_native_menu_requires_replacement() {
        let rt = sample_runtime();
        let initial = menu::build_menu(
            Some(&rt),
            ServiceState::Running,
            None,
            false,
            None,
            "/tmp/.bifrost",
            true,
            &[],
            &[],
            None,
            None,
            false,
            None,
        );
        let next = menu::build_menu(
            Some(&rt),
            ServiceState::Running,
            None,
            false,
            None,
            "/tmp/.bifrost",
            true,
            &[menu::TrayRule {
                target: RuleTarget::Personal {
                    name: "alpha".to_string(),
                },
                enabled: true,
                sort_order: 0,
                managed_group: false,
            }],
            &[],
            None,
            None,
            false,
            None,
        );

        assert_ne!(menu_shape(&initial), menu_shape(&next));
    }

    #[test]
    fn test_quick_menu_snapshot_does_not_wait_for_slow_admin_api() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let admin_url = format!("http://{}/_bifrost/", listener.local_addr().unwrap());
        let data_dir = std::env::temp_dir().join(format!(
            "bifrost-tray-nonblocking-menu-{}",
            std::process::id()
        ));
        let args = TrayArgs {
            data_dir: data_dir.clone(),
            runtime_file: data_dir.join("missing-runtime.json"),
            parent_pid: std::process::id(),
            admin_url: Some(admin_url),
            port: Some(listener.local_addr().unwrap().port()),
            bifrost_bin: Some(PathBuf::from("/tmp/bifrost")),
            start_args: Vec::new(),
        };

        let started = Instant::now();
        let snapshot = load_menu_data_snapshot(&args, ServiceState::Running, false, false);
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_millis(100),
            "quick menu snapshot must not wait for admin API; elapsed={elapsed:?}"
        );
        assert!(snapshot.runtime.is_some());
        assert!(snapshot.rules.is_empty());
        assert!(snapshot.system_proxy.is_none());

        let menu = build_menu_from_snapshot(
            &snapshot,
            ServiceState::Running,
            None,
            false,
            data_dir.to_string_lossy().as_ref(),
            false,
        );
        let status = match &menu[0] {
            menu::MenuEntry::Item(item) => item.label.as_str(),
            menu::MenuEntry::Submenu(_) => panic!("expected status item"),
        };
        assert!(status.starts_with("Bifrost: Running on 127.0.0.1:"));
    }

    #[test]
    fn test_background_menu_refresh_preserves_system_proxy_cache() {
        let data_dir = std::env::temp_dir().join(format!(
            "bifrost-tray-system-proxy-cache-{}",
            std::process::id()
        ));
        let args = TrayArgs {
            data_dir: data_dir.clone(),
            runtime_file: data_dir.join("missing-runtime.json"),
            parent_pid: std::process::id(),
            admin_url: Some("http://127.0.0.1:65535/_bifrost/".to_string()),
            port: Some(65535),
            bifrost_bin: Some(PathBuf::from("/tmp/bifrost")),
            start_args: Vec::new(),
        };
        let cached = menu::SystemProxyMenuState {
            supported: true,
            enabled: true,
        };
        let snapshot = MenuDataSnapshot {
            runtime: runtime_for_menu(&args),
            custom_config: None,
            rules: Vec::new(),
            recent_rule_targets: Vec::new(),
            system_proxy: Some(cached.clone()),
            bin_available: true,
            update_available: None,
            #[cfg(target_os = "macos")]
            system_stats: None,
        };
        let menu_data = Arc::new(Mutex::new(snapshot));
        let generation = AtomicU64::new(0);

        refresh_menu_data_snapshot(
            &args,
            ServiceState::Running,
            &menu_data,
            &generation,
            false,
        );

        let snapshot = clone_menu_data_snapshot(&menu_data);
        assert_eq!(snapshot.system_proxy, Some(cached));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_native_menu_will_open_hook_requests_system_proxy_refresh() {
        struct CountingWillOpen {
            count: std::sync::atomic::AtomicUsize,
        }

        impl NativeStatsMenuWillOpen for CountingWillOpen {
            fn menu_will_open(&self) {
                self.count.fetch_add(1, Ordering::Relaxed);
            }
        }

        let hook = CountingWillOpen {
            count: std::sync::atomic::AtomicUsize::new(0),
        };
        hook.menu_will_open();
        assert_eq!(hook.count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_rule_toggle_url_for_personal_rule() {
        let target = RuleTarget::Personal {
            name: "qa rule".to_string(),
        };
        let url = rule_toggle_url("http://127.0.0.1:8800/_bifrost/", &target, true);
        assert_eq!(
            url,
            "http://127.0.0.1:8800/_bifrost/api/rules/qa%20rule/enable"
        );
    }

    #[test]
    fn test_rule_toggle_url_for_group_rule() {
        let target = RuleTarget::Group {
            group_name: "Team A".to_string(),
            name: "shared/rule".to_string(),
        };
        let url = rule_toggle_url("http://127.0.0.1:8800/_bifrost/", &target, false);
        assert_eq!(
            url,
            "http://127.0.0.1:8800/_bifrost/api/group-rules/Team%20A/shared%2Frule/disable"
        );
    }

    #[test]
    fn test_load_rules_from_admin_marks_active_personal_and_group_rules() {
        let (admin_url, seen, handle) = spawn_test_http_server(vec![
            (
                "HTTP/1.1 200 OK",
                r#"[
                    {"name":"alpha","rule_name":"alpha","group_name":null,"group_id":null},
                    {"name":"stale/group","rule_name":"stale","group_name":"Stale Local","group_id":"grp-stale"}
                ]"#,
            ),
            (
                "HTTP/1.1 200 OK",
                r#"{"code":0,"data":{"list":[{"id":"grp-a","name":"Team A","visibility":0,"level":2},{"id":"grp-master","name":"next-agent","visibility":0,"level":1},{"id":"grp-public","name":"Public","visibility":1,"level":null}]}}"#,
            ),
            (
                "HTTP/1.1 200 OK",
                r#"{"total":1,"rules":[{"name":"shared","rule_count":1,"group_id":"grp-a","group_name":"Team A"}],"variable_conflicts":[],"merged_content":""}"#,
            ),
            (
                "HTTP/1.1 200 OK",
                r#"{"group_id":"grp-a","group_name":"Team A","writable":true,"rules":[{"name":"shared","enabled":true,"sort_order":2,"rule_count":1,"created_at":"","updated_at":"","remote_env_id":null,"remote_user_id":null}]}"#,
            ),
            (
                "HTTP/1.1 200 OK",
                r#"{"group_id":"grp-master","group_name":"next-agent","writable":true,"rules":[{"name":"NextOncall双前端本地开发","enabled":false,"sort_order":1,"rule_count":1,"created_at":"","updated_at":"","remote_env_id":null,"remote_user_id":null}]}"#,
            ),
        ]);

        let rules = load_rules_from_admin(&admin_url);
        handle.join().unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                "GET /_bifrost/api/rules/reference-candidates HTTP/1.1",
                "GET /_bifrost/api/group HTTP/1.1",
                "GET /_bifrost/api/rules/active-summary HTTP/1.1",
                "GET /_bifrost/api/group-rules/grp-a HTTP/1.1",
                "GET /_bifrost/api/group-rules/grp-master HTTP/1.1",
            ]
        );
        assert_eq!(rules.len(), 4);
        let personal = rules
            .iter()
            .find(|rule| {
                rule.target
                    == RuleTarget::Personal {
                        name: "alpha".to_string(),
                    }
            })
            .unwrap();
        assert!(!personal.enabled);
        let group = rules
            .iter()
            .find(|rule| {
                rule.target
                    == RuleTarget::Group {
                        group_name: "Team A".to_string(),
                        name: "shared".to_string(),
                    }
            })
            .unwrap();
        assert!(group.enabled);
        assert!(group.managed_group);

        let master_group = rules
            .iter()
            .find(|rule| {
                rule.target
                    == RuleTarget::Group {
                        group_name: "next-agent".to_string(),
                        name: "NextOncall双前端本地开发".to_string(),
                    }
            })
            .unwrap();
        assert!(!master_group.enabled);
        assert!(master_group.managed_group);

        let hidden_local_group = rules
            .iter()
            .find(|rule| {
                rule.target
                    == RuleTarget::Group {
                        group_name: "Stale Local".to_string(),
                        name: "stale".to_string(),
                    }
            })
            .unwrap();
        assert!(!hidden_local_group.enabled);
        assert!(!hidden_local_group.managed_group);

        let runtime = RuntimeInfo {
            pid: 1234,
            port: 8800,
            socks5_port: None,
            host: Some("127.0.0.1".to_string()),
            started_at_ms: None,
            binary_path: None,
        };
        let menu = menu::build_menu(
            Some(&runtime),
            ServiceState::Running,
            None,
            false,
            None,
            "/tmp/.bifrost",
            true,
            &rules,
            &[],
            None,
            None,
            false,
            None,
        );
        let rules_menu = menu.iter().find_map(|entry| match entry {
            menu::MenuEntry::Submenu(submenu) if submenu.id == "rules_switcher" => Some(submenu),
            _ => None,
        });
        let rules_menu = rules_menu.unwrap();
        assert!(rules_menu.children.iter().any(|entry| matches!(
            entry,
            menu::MenuEntry::Item(item) if item.id == "rules_more"
        )));
        assert!(!rules_menu.children.iter().any(|entry| matches!(
            entry,
            menu::MenuEntry::Submenu(submenu) if submenu.label == "Stale Local"
        )));

        assert!(!rules.iter().any(|rule| {
            rule.target
                == RuleTarget::Group {
                    group_name: "Public".to_string(),
                    name: "public-shared".to_string(),
                }
        }));
    }

    #[test]
    fn test_toggle_single_rule_calls_admin_api_for_disable_then_enable() {
        let (admin_url, seen, handle) = spawn_test_http_server(vec![
            ("HTTP/1.1 200 OK", r#"{"success":true}"#),
            ("HTTP/1.1 200 OK", r#"{"success":true}"#),
        ]);
        let target = RuleTarget::Personal {
            name: "beta".to_string(),
        };
        let enabled_targets = vec![
            RuleTarget::Personal {
                name: "alpha".to_string(),
            },
        ];

        assert!(toggle_single_rule(
            &admin_url,
            &target,
            &enabled_targets,
            false
        ));
        handle.join().unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                "PUT /_bifrost/api/rules/alpha/disable HTTP/1.1",
                "PUT /_bifrost/api/rules/beta/enable HTTP/1.1",
            ]
        );
    }

    #[test]
    fn test_toggle_enabled_rule_calls_admin_api_for_disable_only() {
        let (admin_url, seen, handle) =
            spawn_test_http_server(vec![("HTTP/1.1 200 OK", r#"{"success":true}"#)]);
        let target = RuleTarget::Personal {
            name: "beta".to_string(),
        };
        let enabled_targets = vec![
            RuleTarget::Personal {
                name: "alpha".to_string(),
            },
            target.clone(),
        ];

        assert!(toggle_single_rule(
            &admin_url,
            &target,
            &enabled_targets,
            true
        ));
        handle.join().unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            vec!["PUT /_bifrost/api/rules/beta/disable HTTP/1.1"]
        );
    }

    #[test]
    fn test_record_recent_rule_target_persists_mru_limited_to_five() {
        let dir = tempfile::tempdir().unwrap();
        let targets = [
            RuleTarget::Personal {
                name: "one".to_string(),
            },
            RuleTarget::Personal {
                name: "two".to_string(),
            },
            RuleTarget::Group {
                group_name: "Team".to_string(),
                name: "three".to_string(),
            },
            RuleTarget::Personal {
                name: "four".to_string(),
            },
            RuleTarget::Personal {
                name: "five".to_string(),
            },
            RuleTarget::Personal {
                name: "six".to_string(),
            },
        ];

        for target in &targets {
            record_recent_rule_target(dir.path(), target);
        }
        record_recent_rule_target(dir.path(), &targets[2]);

        assert_eq!(
            load_recent_rule_targets(dir.path()),
            vec![
                targets[2].clone(),
                targets[5].clone(),
                targets[4].clone(),
                targets[3].clone(),
                targets[1].clone(),
            ]
        );
    }

    #[test]
    fn test_remote_group_failure_backoff_suppresses_immediate_retry() {
        clear_remote_group_failure();
        assert!(!remote_group_failure_backoff_active());

        record_remote_group_failure();
        assert!(remote_group_failure_backoff_active());

        clear_remote_group_failure();
        assert!(!remote_group_failure_backoff_active());
    }
