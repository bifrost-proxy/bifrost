use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::time::timeout;

use crate::client::DirectClient;
use crate::{ProxyInstance, TestCase};

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "client_process_curl",
            "resolve curl client_app into traffic records",
            "client_process",
            || async move { verify_client_process("curl").await },
        ),
        TestCase::standalone(
            "client_process_node",
            "resolve node client_app into traffic records",
            "client_process",
            || async move { verify_client_process("node").await },
        ),
        TestCase::standalone(
            "client_process_python",
            "resolve python client_app into traffic records",
            "client_process",
            || async move { verify_client_process("python").await },
        ),
    ]
}

async fn verify_client_process(kind: &str) -> Result<(), String> {
    let proxy_port = portpicker::pick_unused_port().ok_or("Failed to pick proxy port")?;
    let response_delay = if cfg!(target_os = "macos") {
        Duration::ZERO
    } else {
        Duration::from_millis(500)
    };
    let origin = OriginServer::start(response_delay).await?;
    let (_proxy, _admin_state) = ProxyInstance::start_with_admin(proxy_port, vec![], false, true)
        .await
        .map_err(|error| format!("Failed to start proxy with admin: {error}"))?;

    let proxy_url = format!("http://127.0.0.1:{proxy_port}");
    let marker = format!("/client-process-{kind}");
    let target_url = format!(
        "http://{}/{}",
        origin.addr(),
        marker.trim_start_matches('/')
    );

    run_external_client(kind, &proxy_url, &target_url).await?;
    origin.finish().await?;

    let client_app = wait_for_client_app(proxy_port, &marker).await?;
    let normalized = client_app.to_lowercase();
    let expected = match kind {
        "curl" => "curl",
        "node" => "node",
        "python" => "python",
        _ => return Err(format!("unsupported client kind: {kind}")),
    };

    if !normalized.contains(expected) {
        return Err(format!(
            "expected client_app for {kind} to contain {expected:?}, got {client_app:?}"
        ));
    }

    Ok(())
}

async fn run_external_client(kind: &str, proxy_url: &str, target_url: &str) -> Result<(), String> {
    let mut command = match kind {
        "curl" => {
            let mut command = Command::new(resolve_executable("curl"));
            command.args(["-sS", "--noproxy", "", "-x", proxy_url, target_url]);
            command
        }
        "node" => {
            let mut command = Command::new(resolve_executable("node"));
            command.args([
                "-e",
                "const http = require('http'); const proxy = new URL(process.env.TEST_PROXY_URL); const target = new URL(process.env.TEST_TARGET_URL); const req = http.request({ host: proxy.hostname, port: proxy.port, method: 'GET', path: process.env.TEST_TARGET_URL, headers: { Host: target.host, Connection: 'close' } }, (res) => { res.resume(); res.on('end', () => { req.destroy(); process.exit(0); }); }); req.setTimeout(10000, () => { console.error('node client timeout'); req.destroy(new Error('timeout')); process.exit(1); }); req.on('error', (err) => { console.error(err); process.exit(1); }); req.end();",
            ]);
            command.env("TEST_PROXY_URL", proxy_url);
            command.env("TEST_TARGET_URL", target_url);
            command
        }
        "python" => {
            let python_cmd = if cfg!(target_os = "windows") {
                resolve_executable("python")
            } else {
                resolve_executable("python3")
            };
            let mut command = Command::new(python_cmd);
            command.args([
                "-c",
                "import http.client, os, sys, urllib.parse; proxy = urllib.parse.urlparse(os.environ['TEST_PROXY_URL']); target = urllib.parse.urlparse(os.environ['TEST_TARGET_URL']); conn = http.client.HTTPConnection(proxy.hostname, proxy.port, timeout=10); conn.request('GET', os.environ['TEST_TARGET_URL'], headers={'Host': target.netloc, 'Connection': 'close'}); resp = conn.getresponse(); resp.read(); conn.close(); sys.exit(0)",
            ]);
            command.env("TEST_PROXY_URL", proxy_url);
            command.env("TEST_TARGET_URL", target_url);
            command
        }
        _ => return Err(format!("unsupported client kind: {kind}")),
    };

    command.stdout(Stdio::null());
    command.stderr(Stdio::piped());
    command.stdin(Stdio::null());
    command.kill_on_drop(true);

    let child = command
        .spawn()
        .map_err(|error| format!("failed to start {kind} client: {error}"))?;
    let output = match timeout(Duration::from_secs(15), child.wait_with_output()).await {
        Ok(result) => {
            result.map_err(|error| format!("failed to wait for {kind} client: {error}"))?
        }
        Err(_) => {
            return Err(format!(
                "{kind} client timed out after 15s via {:?}",
                resolve_executable(kind)
            ))
        }
    };
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{kind} client exited with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn resolve_executable(command: &str) -> String {
    let (resolver, args): (&str, &[&str]) = if cfg!(target_os = "windows") {
        ("where.exe", &[command])
    } else {
        ("which", &["-a", command])
    };

    let output = std::process::Command::new(resolver).args(args).output();
    let Ok(output) = output else {
        return command.to_string();
    };
    if !output.status.success() {
        return command.to_string();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let is_mise_shim = |p: &str| p.contains("/mise/shims/") || p.contains("\\mise\\shims\\");
    stdout
        .lines()
        .map(str::trim)
        .find(|path| !path.is_empty() && !is_mise_shim(path))
        .or_else(|| stdout.lines().map(str::trim).find(|path| !path.is_empty()))
        .unwrap_or(command)
        .to_string()
}

async fn wait_for_client_app(proxy_port: u16, marker: &str) -> Result<String, String> {
    let direct = DirectClient::new().map_err(|error| error.to_string())?;
    let list_url = format!("http://127.0.0.1:{proxy_port}/_bifrost/api/traffic?limit=20");

    for _ in 0..40 {
        let list_json = direct
            .get_json(&list_url)
            .await
            .map_err(|error| error.to_string())?;

        if let Some(client_app) = find_client_app(&list_json, marker) {
            return Ok(client_app);
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let list_json = direct
        .get_json(&list_url)
        .await
        .map_err(|error| error.to_string())?;
    Err(format!(
        "timed out waiting for traffic record with marker {marker}; records={}",
        summarize_records(&list_json)
    ))
}

fn find_client_app(list_json: &Value, marker: &str) -> Option<String> {
    let records = list_json.get("records")?.as_array()?;
    records.iter().find_map(|record| {
        let path = record
            .get("path")
            .or_else(|| record.get("p"))
            .and_then(Value::as_str)?;
        if !path.contains(marker) {
            return None;
        }

        record
            .get("client_app")
            .or_else(|| record.get("capp"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

fn summarize_records(list_json: &Value) -> String {
    let Some(records) = list_json.get("records").and_then(Value::as_array) else {
        return list_json.to_string();
    };

    records
        .iter()
        .map(|record| {
            let path = record
                .get("path")
                .or_else(|| record.get("p"))
                .and_then(Value::as_str)
                .unwrap_or("<missing-path>");
            let client_app = record
                .get("client_app")
                .or_else(|| record.get("capp"))
                .and_then(Value::as_str)
                .unwrap_or("<none>");
            format!("{path} app={client_app}")
        })
        .collect::<Vec<_>>()
        .join(" || ")
}

struct OriginServer {
    addr: std::net::SocketAddr,
    handle: tokio::task::JoinHandle<Result<(), String>>,
}

impl OriginServer {
    async fn start(response_delay: Duration) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| format!("bind origin listener: {error}"))?;
        let addr = listener
            .local_addr()
            .map_err(|error| format!("origin local_addr: {error}"))?;

        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener
                .accept()
                .await
                .map_err(|error| format!("accept origin request: {error}"))?;

            let mut buffer = [0u8; 4096];
            let _ = socket
                .read(&mut buffer)
                .await
                .map_err(|error| format!("read origin request: {error}"))?;

            if !response_delay.is_zero() {
                tokio::time::sleep(response_delay).await;
            }

            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .map_err(|error| format!("write origin response: {error}"))?;

            Ok(())
        });

        Ok(Self { addr, handle })
    }

    fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }

    async fn finish(self) -> Result<(), String> {
        self.handle
            .await
            .map_err(|error| format!("join origin server: {error}"))?
    }
}
