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
            None,
        );
        let status = match &menu[0] {
            menu::MenuEntry::Item(item) => item.label.as_str(),
            menu::MenuEntry::Submenu(_) => panic!("expected status item"),
        };
        assert_eq!(status, "Bifrost: Running on 127.0.0.1:9900");
    }

    #[test]
    fn test_pure_tray_icon_event_does_not_rebuild_native_menu() {
        assert!(!should_rebuild_native_menu(
            false, false, false, false, false
        ));
    }

    #[test]
    fn test_menu_update_still_runs_for_real_state_and_rule_triggers() {
        assert!(should_rebuild_native_menu(true, false, false, false, false));
        assert!(should_rebuild_native_menu(false, true, false, false, false));
        assert!(should_rebuild_native_menu(false, false, true, false, false));
        assert!(should_rebuild_native_menu(false, false, false, true, false));
        assert!(should_rebuild_native_menu(false, false, false, false, true));
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
        let snapshot = load_menu_data_snapshot(&args, ServiceState::Running, false);
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
        );
        let status = match &menu[0] {
            menu::MenuEntry::Item(item) => item.label.as_str(),
            menu::MenuEntry::Submenu(_) => panic!("expected status item"),
        };
        assert!(status.starts_with("Bifrost: Running on 127.0.0.1:"));
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
    fn test_select_single_rule_calls_admin_api_for_disable_then_enable() {
        let (admin_url, seen, handle) = spawn_test_http_server(vec![
            ("HTTP/1.1 200 OK", r#"{"success":true}"#),
            ("HTTP/1.1 200 OK", r#"{"success":true}"#),
        ]);
        let target = RuleTarget::Personal {
            name: "beta".to_string(),
        };
        let all_targets = vec![
            RuleTarget::Personal {
                name: "alpha".to_string(),
            },
            target.clone(),
        ];

        assert!(select_single_rule(&admin_url, &target, &all_targets));
        handle.join().unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                "PUT /_bifrost/api/rules/alpha/disable HTTP/1.1",
                "PUT /_bifrost/api/rules/beta/enable HTTP/1.1",
            ]
        );
    }