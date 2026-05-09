#[cfg(target_os = "macos")]
use super::macos::describe_process_tcp_sockets;
use super::*;
#[cfg(target_os = "macos")]
use std::env;
use std::net::{IpAddr, Ipv4Addr};
#[cfg(target_os = "macos")]
use std::path::PathBuf;

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
