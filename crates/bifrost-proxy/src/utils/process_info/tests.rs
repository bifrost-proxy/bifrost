#[cfg(target_os = "macos")]
use super::macos::describe_process_tcp_sockets;
use super::*;
#[cfg(target_os = "macos")]
use std::env;
use std::net::{IpAddr, Ipv4Addr};
#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

#[cfg(target_os = "macos")]
use std::process::Stdio;

#[cfg(target_os = "macos")]
use tokio::io::AsyncWriteExt;

#[cfg(target_os = "macos")]
use tokio::process::Command;

#[test]
fn test_format_client_info_with_process() {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345);
    let process = ClientProcess {
        pid: 1234,
        name: "Chrome".to_string(),
        path: Some("/Applications/Chrome.app".to_string()),
    };
    let result = format_client_info(&addr, Some(&process));
    assert_eq!(result, "Chrome");
}

#[test]
fn test_format_client_info_without_process() {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 12345);
    let result = format_client_info(&addr, None);
    assert_eq!(result, "192.168.1.100");
}

#[test]
fn test_process_resolver_cache() {
    let resolver = ProcessResolver::new();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 54321);

    let _ = resolver.resolve(&addr);

    let cached = resolver.get_from_cache(&ConnKey::from_peer_addr(&addr));
    assert!(cached.is_some());
}

#[test]
fn test_process_resolver_cached_lookup_miss() {
    let resolver = ProcessResolver::new();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 54321);

    let cached = resolver.resolve_cached(&addr);
    assert!(cached.is_none());
}

#[test]
fn process_resolver_diagnostics_distinguish_positive_and_negative_cache_hits() {
    let resolver = ProcessResolver::new();
    let positive = ConnKey::from_peer_addr(&"127.0.0.1:54331".parse().unwrap());
    let negative = ConnKey::from_peer_addr(&"127.0.0.1:54332".parse().unwrap());
    resolver.update_cache(
        positive,
        Some(ClientProcess {
            pid: 123,
            name: "diagnostics-client".to_string(),
            path: None,
        }),
    );
    resolver.update_cache(negative, None);

    assert!(resolver.get_from_cache(&positive).unwrap().is_some());
    assert!(resolver.get_from_cache(&negative).unwrap().is_none());
    let snapshot = resolver.diagnostics().snapshot();
    assert_eq!(snapshot.positive_cache_hits_total, 1);
    assert_eq!(snapshot.negative_cache_hits_total, 1);
}

#[test]
fn connection_owned_lookup_ignores_cross_connection_positive_but_keeps_short_negative() {
    let resolver = ProcessResolver::new();
    let positive = ConnKey::from_connection(
        &"127.0.0.1:54333".parse().unwrap(),
        &"127.0.0.1:9900".parse().unwrap(),
    );
    let negative = ConnKey::from_connection(
        &"127.0.0.1:54334".parse().unwrap(),
        &"127.0.0.1:9900".parse().unwrap(),
    );
    resolver.update_cache(
        positive,
        Some(ClientProcess {
            pid: 321,
            name: "old-port-owner".to_string(),
            path: None,
        }),
    );
    resolver.update_cache(negative, None);

    assert!(resolver
        .get_from_cache_for_connection_owned(&positive)
        .is_none());
    assert!(matches!(
        resolver.get_from_cache_for_connection_owned(&negative),
        Some(None)
    ));
    assert_eq!(
        resolver.get_from_cache(&positive).unwrap().unwrap().pid,
        321
    );
}

#[test]
fn process_resolver_diagnostics_record_snapshot_refresh_cost() {
    let resolver = ProcessResolver::new();
    let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();

    let _ = resolver.resolve(&addr);

    let snapshot = resolver.diagnostics().snapshot();
    assert_eq!(snapshot.lookup_requests_total, 1);
    assert_eq!(snapshot.snapshot_misses_total, 1);
    assert_eq!(snapshot.snapshot_refreshes_total, 1);
    assert_eq!(snapshot.resolved_total + snapshot.unresolved_total, 1);
    assert!(snapshot.scan_duration_max_us <= snapshot.scan_duration_total_us);
}

#[test]
fn concurrent_connections_share_one_snapshot_generation() {
    let keys = (0..16)
        .map(|offset| {
            ConnKey::from_connection(
                &format!("127.0.0.1:{}", 45_000 + offset).parse().unwrap(),
                &"127.0.0.1:9900".parse().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let connections_to_pids = keys
        .iter()
        .enumerate()
        .map(|(index, key)| (*key, 10_000 + index as u32))
        .collect::<HashMap<_, _>>();
    let scan_count = Arc::new(AtomicUsize::new(0));
    let scan_count_for_scanner = Arc::clone(&scan_count);
    let resolver = Arc::new(ProcessResolver::with_socket_pid_scanner(move || {
        scan_count_for_scanner.fetch_add(1, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(30));
        SocketPidMapScan {
            connections_to_pids: connections_to_pids.clone(),
            scanned_pids: 16,
            scanned_fds: 16,
            failed: false,
        }
    }));
    let barrier = Arc::new(Barrier::new(keys.len()));

    let workers = keys
        .into_iter()
        .enumerate()
        .map(|(index, key)| {
            let resolver = Arc::clone(&resolver);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                assert_eq!(resolver.lookup_pid(&key), Some(10_000 + index as u32));
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }

    assert_eq!(scan_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        resolver.diagnostics().snapshot().snapshot_refreshes_total,
        1
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn failed_socket_scan_reports_failure_without_partial_counts() {
    let scan = failed_socket_pid_map_scan(&"synthetic netstat failure");

    assert!(scan.failed);
    assert!(scan.connections_to_pids.is_empty());
    assert_eq!(scan.scanned_pids, 0);
    assert_eq!(scan.scanned_fds, 0);
}

#[tokio::test]
async fn test_process_resolver_async_returns_cached_hit() {
    let resolver = ProcessResolver::new();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 54321);
    let process = ClientProcess {
        pid: 1234,
        name: "Chrome".to_string(),
        path: Some("/Applications/Chrome.app".to_string()),
    };

    resolver.update_cache(ConnKey::from_peer_addr(&addr), Some(process.clone()));

    let resolved = resolver.resolve_async(addr, 3, 10).await;
    assert_eq!(
        resolved.as_ref().map(|process| process.name.as_str()),
        Some("Chrome")
    );
    assert_eq!(resolved.as_ref().map(|process| process.pid), Some(1234));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_process_resolution_timeout_returns_none_and_negative_caches() {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 54322);
    let key = ConnKey::from_peer_addr(&addr);
    let task = tokio::task::spawn_blocking(|| {
        std::thread::sleep(Duration::from_millis(250));
        Some(ClientProcess {
            pid: 4321,
            name: "slow-client".to_string(),
            path: None,
        })
    });

    let resolved = wait_for_process_resolution_with_timeout(task, key, Duration::from_millis(25))
        .await
        .expect("timeout path should not fail the join");

    assert!(resolved.is_none());
    assert!(matches!(PROCESS_RESOLVER.get_from_cache(&key), Some(None)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_async_process_resolution_respects_negative_cache() {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 54323);
    let key = ConnKey::from_peer_addr(&addr);
    PROCESS_RESOLVER.update_cache(key, None);

    let start = Instant::now();
    let resolved = resolve_client_process_async_with_retry(&addr, 20, 50).await;

    assert!(resolved.is_none());
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "negative cache should avoid retry sleeps and blocking resolution"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_process_resolution_concurrency_wait_timeout_does_not_negative_cache() {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 54324);
    let key = ConnKey::from_peer_addr(&addr);
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started_for_task = Arc::clone(&started);

    let resolved = resolve_with_limited_blocking_task_for_semaphore(
        key,
        Arc::new(Semaphore::new(0)),
        0,
        Duration::from_millis(25),
        move || {
            started_for_task.store(true, std::sync::atomic::Ordering::SeqCst);
            Some(ClientProcess {
                pid: 4322,
                name: "should-not-run".to_string(),
                path: None,
            })
        },
    )
    .await
    .expect("saturated path should not fail the join");

    assert!(resolved.is_none());
    assert!(!started.load(std::sync::atomic::Ordering::SeqCst));
    assert!(PROCESS_RESOLVER.get_from_cache(&key).is_none());
}

#[test]
fn test_process_resolver_retry_caches_miss() {
    let resolver = ProcessResolver::new();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 1);

    let resolved = resolver.resolve_with_retry(&addr, 0, 0);
    assert!(resolved.is_none());
    assert!(matches!(
        resolver.get_from_cache(&ConnKey::from_peer_addr(&addr)),
        Some(None)
    ));
}

#[test]
fn test_conn_key_uses_proxy_addr() {
    let peer_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 51000);
    let proxy_a = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
    let proxy_b = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9090);

    assert_ne!(
        ConnKey::from_connection(&peer_addr, &proxy_a),
        ConnKey::from_connection(&peer_addr, &proxy_b)
    );
}

#[test]
fn client_process_display_name_and_default_resolver_are_stable() {
    let process = ClientProcess {
        pid: 77,
        name: "coverage-client".to_string(),
        path: None,
    };
    assert_eq!(process.display_name(), "coverage-client");
    let resolver = ProcessResolver::default();
    assert_eq!(resolver.cache.len(), 0);
}

#[tokio::test]
async fn async_resolvers_cover_connection_cache_and_non_loopback_shortcuts() {
    let resolver = ProcessResolver::new();
    let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 54001);
    let local = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9900);
    let process = ClientProcess {
        pid: 88,
        name: "cached-connection".to_string(),
        path: Some("/tmp/cached".to_string()),
    };
    let key = ConnKey::from_connection(&peer, &local);
    resolver.update_cache(key, Some(process.clone()));
    assert_eq!(
        resolver
            .resolve_cached_for_connection(&peer, &local)
            .unwrap()
            .pid,
        88
    );
    assert_eq!(
        resolver
            .resolve_async_for_connection(peer, local, 0, 0)
            .await
            .unwrap()
            .name,
        "cached-connection"
    );

    let remote = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 54002);
    assert!(resolver.resolve_async(remote, 0, 0).await.is_none());
    assert!(resolver
        .resolve_async_for_connection(remote, local, 0, 0)
        .await
        .is_none());
}

#[test]
fn bounded_ttl_cache_expires_entries_and_hard_caps_live_cardinality() {
    let resolver = ProcessResolver::new();
    let key = ConnKey::from_peer_addr(&SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 55000));
    resolver.cache.insert(
        key,
        Some(Arc::new(ClientProcess {
            pid: 99,
            name: "expired".to_string(),
            path: None,
        })),
        Instant::now() - Duration::from_secs(1),
    );
    assert!(resolver.get_from_cache(&key).is_none());

    let cache = BoundedTtlCache::new(64, 4);
    for value in 0..10_000_u64 {
        cache.insert(value, value, Instant::now() + Duration::from_secs(30));
    }
    assert!(cache.len() <= 64);
    assert!(cache.evictions_total() >= 9_936);
}

#[test]
fn bounded_ttl_cache_replacement_ignores_stale_expiry_markers() {
    let cache = BoundedTtlCache::new(64, 4);
    let now = Instant::now();
    cache.insert(7_u64, "old", now + Duration::from_millis(5));
    cache.insert(7_u64, "new", now + Duration::from_secs(30));

    assert_eq!(cache.cleanup_expired(now + Duration::from_millis(10)), 0);
    assert_eq!(cache.get(&7, now + Duration::from_millis(10)), Some("new"));
    assert_eq!(cache.len(), 1);
}

#[test]
fn bounded_ttl_cache_concurrent_inserts_remain_within_hard_capacity() {
    let cache = Arc::new(BoundedTtlCache::new(256, 8));
    let workers = (0..8_u64)
        .map(|worker| {
            let cache = Arc::clone(&cache);
            thread::spawn(move || {
                for offset in 0..2_000_u64 {
                    let key = worker * 2_000 + offset;
                    cache.insert(key, key, Instant::now() + Duration::from_secs(30));
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }

    assert!(cache.len() <= 256);
    assert!(cache.evictions_total() >= 15_744);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn limited_blocking_resolution_covers_success_closed_and_join_error() {
    let key = ConnKey::from_peer_addr(&SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 56000));
    let resolved = resolve_with_limited_blocking_task_for_semaphore(
        key,
        Arc::new(Semaphore::new(1)),
        1,
        Duration::from_secs(1),
        || {
            Some(ClientProcess {
                pid: 101,
                name: "resolved".to_string(),
                path: None,
            })
        },
    )
    .await
    .unwrap();
    assert_eq!(resolved.unwrap().pid, 101);

    let closed = Arc::new(Semaphore::new(1));
    closed.close();
    assert!(resolve_with_limited_blocking_task_for_semaphore(
        key,
        closed,
        1,
        Duration::from_secs(1),
        || -> Option<ClientProcess> { panic!("closed semaphore must not run resolver") },
    )
    .await
    .unwrap()
    .is_none());

    let task = tokio::spawn(std::future::pending::<Option<ClientProcess>>());
    task.abort();
    let error = wait_for_process_resolution_with_timeout(task, key, Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(error.is_cancelled());
}

#[test]
fn app_policy_retry_config_is_nonzero_and_format_uses_path_fallback_name() {
    let (retries, delay_ms) = app_policy_process_resolution_retry_config();
    assert!(retries > 0);
    assert!(delay_ms > 0);
    let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 57000);
    let process = ClientProcess {
        pid: 102,
        name: "display".to_string(),
        path: Some("/tmp/display".to_string()),
    };
    assert_eq!(format_client_info(&peer, Some(&process)), "display");
}

#[cfg(target_os = "macos")]
fn find_test_program(program: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if program.contains(std::path::MAIN_SEPARATOR) {
        candidates.push(PathBuf::from(program));
    } else {
        if let Some(path) = env::var_os("PATH") {
            candidates.extend(env::split_paths(&path).map(|entry| entry.join(program)));
        }

        if let Some(home) = env::var_os("HOME") {
            let home = PathBuf::from(home);
            candidates.push(home.join(".local/share/mise/shims").join(program));
            candidates.push(home.join(".mise/shims").join(program));
            candidates.push(home.join(".asdf/shims").join(program));
        }

        candidates.push(PathBuf::from("/opt/homebrew/bin").join(program));
        candidates.push(PathBuf::from("/usr/local/bin").join(program));
        candidates.push(PathBuf::from("/usr/bin").join(program));
    }

    candidates.into_iter().find(|candidate| candidate.is_file())
}

#[cfg(target_os = "macos")]
async fn resolve_process_from_external_client(
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<ClientProcess, String> {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|error| format!("bind listener: {error}"))?;
    let local_addr = listener
        .local_addr()
        .map_err(|error| format!("listener local_addr: {error}"))?;
    let url = format!("http://{local_addr}/resolver-test");
    let resolved_program = find_test_program(program)
        .ok_or_else(|| format!("unable to locate executable for test program {program}"))?;

    let mut command = Command::new(&resolved_program);
    command.args(args.iter().map(|arg| arg.replace("{url}", &url)));
    command.stdout(Stdio::null());
    command.stderr(Stdio::piped());
    // Ensure the spawned client connects DIRECTLY to our local listener.
    // Some environments export proxy variables (e.g. http_proxy) pointing to `bifrost`,
    // which would make the peer process be the proxy instead of the intended client.
    for key in [
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
    ] {
        command.env_remove(key);
    }
    command.env("NO_PROXY", "*");
    command.env("no_proxy", "*");
    for (key, value) in envs {
        command.env(key, value.replace("{url}", &url));
    }

    let child = command
        .spawn()
        .map_err(|error| format!("spawn {program}: {error}"))?;
    let child_pid = child.id();

    let (mut stream, peer_addr) = listener
        .accept()
        .await
        .map_err(|error| format!("accept connection from {program}: {error}"))?;

    let resolved =
        resolve_client_process_async_for_connection_with_retry(&peer_addr, &local_addr, 20, 50)
            .await
            .ok_or_else(|| {
                let socket_dump = child_pid
                    .map(|pid| describe_process_tcp_sockets(pid).join(" | "))
                    .unwrap_or_else(|| "child pid unavailable".to_string());
                format!(
                    "resolver returned None for {program} peer={peer_addr} local={local_addr}; sockets={socket_dump}"
                )
            })?;

    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
        .await
        .map_err(|error| format!("write response to {program}: {error}"))?;

    let output = child
        .wait_with_output()
        .await
        .map_err(|error| format!("wait for {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} exited with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(resolved)
}

#[cfg(target_os = "macos")]
fn assert_process_name_matches(process: &ClientProcess, expected_tokens: &[&str]) {
    let process_name = process.name.to_lowercase();
    let process_path = process.path.as_deref().unwrap_or_default().to_lowercase();
    assert!(
        expected_tokens
            .iter()
            .any(|token| process_name.contains(token) || process_path.contains(token)),
        "expected process {:?} / {:?} to match one of {:?}",
        process.name,
        process.path,
        expected_tokens
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn test_process_resolver_detects_curl_client() {
    let process = resolve_process_from_external_client("curl", &["-sS", "{url}"], &[])
        .await
        .expect("resolve curl client process");

    assert_process_name_matches(&process, &["curl"]);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn test_process_resolver_detects_node_client() {
    let process = resolve_process_from_external_client(
        "node",
        &[
            "-e",
            "const http = require('http'); const url = process.env.TEST_URL; http.get(url, (res) => { res.resume(); res.on('end', () => process.exit(0)); }).on('error', (err) => { console.error(err); process.exit(1); });",
        ],
        &[("TEST_URL", "{url}")],
    )
    .await
    .expect("resolve node client process");

    assert_process_name_matches(&process, &["node"]);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn test_process_resolver_detects_python_client() {
    let process = resolve_process_from_external_client(
        "python3",
        &[
            "-c",
            "import os, sys, urllib.request; urllib.request.urlopen(os.environ['TEST_URL']).read(); sys.exit(0)",
        ],
        &[("TEST_URL", "{url}")],
    )
    .await
    .expect("resolve python client process");

    assert_process_name_matches(&process, &["python"]);
}

#[cfg(target_os = "macos")]
#[test]
fn coverage_90_macos_process_introspection_covers_current_and_invalid_pids() {
    let pid = std::process::id();
    let (name, path) = get_process_info(pid);
    assert!(!name.is_empty());
    assert!(path.is_some());
    assert!(get_process_name_macos(pid).is_some());
    assert!(get_process_path_macos(pid).is_some());
    assert!(get_process_name_macos(u32::MAX).is_none());
    assert!(get_process_path_macos(u32::MAX).is_none());

    let _ = super::macos::lookup_socket_pid_map_macos();
    let _ = describe_process_tcp_sockets(pid);
    let invalid = describe_process_tcp_sockets(u32::MAX);
    assert!(!invalid.is_empty());
}

#[tokio::test]
async fn coverage_90_public_process_resolution_wrappers_are_safe_for_remote_clients() {
    let remote: SocketAddr = "192.0.2.10:54321".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:9900".parse().unwrap();

    let _ = resolve_client_process(&remote);
    let _ = resolve_client_process_for_connection(&remote, &local);
    let _ = resolve_client_process_cached(&remote);
    let _ = resolve_client_process_cached_for_connection(&remote, &local);
    let _ = resolve_client_process_with_retry(&remote, 0, 0);
    let _ = resolve_client_process_for_connection_with_retry(&remote, &local, 0, 0);
    assert!(resolve_client_process_async(&remote).await.is_none());
    assert!(resolve_client_process_async_for_connection(&remote, &local)
        .await
        .is_none());
    assert!(resolve_client_process_async_with_retry(&remote, 0, 0)
        .await
        .is_none());
    assert!(
        resolve_client_process_async_for_connection_with_retry(&remote, &local, 0, 0)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn coverage_90_background_process_resolution_always_finishes() {
    let peer: SocketAddr = "192.0.2.20:12345".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:9900".parse().unwrap();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    spawn_async_process_resolver_with_finish(
        peer,
        local,
        "coverage-record".into(),
        |_id, _process| panic!("remote peer must not resolve"),
        move || {
            let _ = finished_tx.send(());
        },
    );
    tokio::time::timeout(Duration::from_secs(5), finished_rx)
        .await
        .expect("background resolver timeout")
        .expect("finish callback dropped");

    spawn_async_process_resolver(peer, local, "fire-and-forget".into(), |_id, _process| {});
}

#[test]
fn coverage_90_cleanup_expired_removes_all_cache_layers() {
    let resolver = ProcessResolver::new();
    let peer: SocketAddr = "127.0.0.1:54329".parse().unwrap();
    let key = ConnKey::from_peer_addr(&peer);
    resolver
        .cache
        .insert(key, None, Instant::now() - Duration::from_secs(1));
    resolver.pid_cache.insert(
        42,
        Arc::new(ClientProcess {
            pid: 42,
            name: "expired".to_string(),
            path: None,
        }),
        Instant::now() - Duration::from_secs(1),
    );
    *resolver.socket_snapshot.write().unwrap() = Some(SocketSnapshot {
        connections_to_pids: HashMap::from([(key, 42)]),
        refreshed_at: Instant::now() - Duration::from_secs(2),
        expires_at: Instant::now() - Duration::from_secs(1),
    });
    resolver.cleanup_expired();
    assert_eq!(resolver.cache.len(), 0);
    assert_eq!(resolver.pid_cache.len(), 0);
    assert!(resolver.socket_snapshot.read().unwrap().is_none());
}

#[tokio::test]
async fn coverage_90_local_async_wrapper_runs_limited_blocking_resolution() {
    let peer: SocketAddr = "127.0.0.1:54331".parse().unwrap();
    let _ = resolve_client_process_async_with_retry(&peer, 0, 0).await;
}
