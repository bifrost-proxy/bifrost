use std::collections::BTreeMap;
use std::net::TcpStream;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use portpicker::pick_unused_port;
use reqwest::{Client, Response, StatusCode};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{json, Map, Value};
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};

use crate::runner::TestCase;

const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

struct SyncServerGuard {
    child: Child,
    _data_dir: TempDir,
}

impl Drop for SyncServerGuard {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

pub fn get_all_tests() -> Vec<TestCase> {
    vec![TestCase::standalone(
        "remote_invoke_pop_pair_claim_lookup_open_revoke",
        "v5 PoP protects claim, lookup, open, and revoke without exposing grant crypto in approved SSE",
        "remote_invoke",
        || async { run_remote_invoke_pop_flow().await },
    )]
}

async fn run_remote_invoke_pop_flow() -> Result<(), String> {
    let (server, base_url) = start_sync_server().await?;
    let _server = server;
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let suffix = now_millis();
    let user_id = format!("ri-pop-owner-{suffix}");
    let client_instance_id = format!("ri-pop-client-{suffix}");
    let pair_code = format!("{:06}", suffix % 1_000_000);

    let user_token = register_user(&http, &base_url, &user_id).await?;
    let client_keys = generate_ed25519_keypair()?;
    let client_pubkey = ed25519_pubkey_b64(&client_keys);
    let client_auth_token = register_client(
        &http,
        &base_url,
        &client_instance_id,
        &client_pubkey,
        &client_keys,
        &user_token,
    )
    .await?;

    let mut client_stream = open_sse(
        &http,
        &format!(
            "{base_url}/v4/remote-invoke/client/stream?client_instance_id={}&stream_id=ri-pop-stream&client_auth_token={}",
            urlencoding::encode(&client_instance_id),
            urlencoding::encode(&client_auth_token),
        ),
    )
    .await?;
    let _ = next_sse_event(&mut client_stream, Some("client_hello_ack")).await?;

    post_ok(
        &http,
        &format!("{base_url}/v4/remote-invoke/client/pair-code"),
        json!({
            "client_instance_id": client_instance_id,
            "pair_code": pair_code,
            "expires_at": (now_millis() + 120_000) as u64,
        }),
        Some(&client_auth_token),
    )
    .await?;

    let caller_keys = generate_ed25519_keypair()?;
    let start = post_ok(
        &http,
        &format!("{base_url}/v5/remote-invoke/pairings/start"),
        json!({
            "pair_code": pair_code,
            "caller_pubkey": ed25519_pubkey_b64(&caller_keys),
            "caller_info": {
                "fingerprint": "caller-pop-e2e",
                "display_name": "PoP E2E caller"
            },
            "caller_ephemeral_pub": random_x25519_pub_b64()?,
        }),
        None,
    )
    .await?;
    let pairing_id = data_str(&start, "pairing_id")?.to_string();
    let watch_token = data_str(&start, "watch_token")?.to_string();

    let watch_url = format!(
        "{base_url}/v5/remote-invoke/pairings/{}/watch?watch_token={}",
        pairing_id,
        urlencoding::encode(&watch_token),
    );
    let mut watcher_a = open_sse(&http, &watch_url).await?;
    let mut watcher_b = open_sse(&http, &watch_url).await?;
    let _ = next_sse_event(&mut watcher_a, Some("connected")).await?;
    let _ = next_sse_event(&mut watcher_b, Some("connected")).await?;

    let client_ephemeral_pub = random_x25519_pub_b64()?;
    post_ok(
        &http,
        &format!("{base_url}/v4/remote-invoke/client/grants/{pairing_id}/decision"),
        json!({
            "decision": "approve",
            "grant_mode": "permanent",
            "grant_scope": "remote_shell_exec",
            "client_instance_id": client_instance_id,
            "client_ephemeral_pub": client_ephemeral_pub,
        }),
        Some(&client_auth_token),
    )
    .await?;

    let approved_a = next_sse_event(&mut watcher_a, Some("approved")).await?;
    let approved_b = next_sse_event(&mut watcher_b, Some("approved")).await?;
    assert_no_approved_crypto_context(&approved_a)?;
    assert_no_approved_crypto_context(&approved_b)?;
    let claim_token = approved_a
        .get("claim_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "approved event missing claim_token".to_string())?;

    let claim_caller_ephemeral = random_x25519_pub_b64()?;
    let claim = post_ok(
        &http,
        &format!("{base_url}/v5/remote-invoke/grants/claim"),
        sign_pop_body(
            json!({
                "client_instance_id": client_instance_id,
                "pair_code": pair_code,
                "claim_token": claim_token,
                "caller_ephemeral_pub": claim_caller_ephemeral,
            }),
            &caller_keys,
        )?,
        None,
    )
    .await?;
    let claim_summary = claim
        .get("data")
        .and_then(|data| data.get("grant_summary"))
        .ok_or_else(|| "claim response missing grant_summary".to_string())?;
    if claim_summary
        .get("client_ephemeral_pub")
        .and_then(Value::as_str)
        != Some(client_ephemeral_pub.as_str())
    {
        return Err("claim response did not return client_ephemeral_pub".to_string());
    }

    let lookup = post_ok(
        &http,
        &format!("{base_url}/v5/remote-invoke/grants/lookup"),
        sign_pop_body(
            json!({
                "client_instance_id": client_instance_id,
                "caller_ephemeral_pub": claim_caller_ephemeral,
            }),
            &caller_keys,
        )?,
        None,
    )
    .await?;
    let grant_session_token = data_str(&lookup, "grant_session_token")?.to_string();
    let lookup_summary = lookup
        .get("data")
        .and_then(|data| data.get("grant_summary"))
        .ok_or_else(|| "lookup response missing grant_summary".to_string())?;
    if lookup_summary
        .get("client_ephemeral_pub")
        .and_then(Value::as_str)
        != Some(client_ephemeral_pub.as_str())
    {
        return Err("lookup response did not return client_ephemeral_pub".to_string());
    }

    let open_body = sign_pop_body(
        json!({
            "client_instance_id": client_instance_id,
            "command_kind": "shell.exec",
            "command_encrypted": {
                "version": 2,
                "nonce": "ri-pop-open-nonce",
                "ciphertext": "ri-pop-command",
                "tag": "ri-pop-tag"
            },
            "command_summary": {
                "command_preview": "echo ri-pop"
            },
            "timeout_hint_ms": 5000
        }),
        &caller_keys,
    )?;
    if open_body.get("grant_id").is_some() {
        return Err("v5 open body unexpectedly includes grant_id".to_string());
    }
    let open = post_ok_auth(
        &http,
        &format!("{base_url}/v5/remote-invoke/calls/open"),
        open_body,
        &grant_session_token,
    )
    .await?;
    let call_id = data_str(&open, "call_id")?.to_string();
    let relay_token = data_str(&open, "relay_token")?.to_string();

    let call_open = next_sse_event(&mut client_stream, Some("call_open")).await?;
    if call_open.get("grant_id").and_then(Value::as_str).is_none() {
        return Err("client call_open event missing server-side grant_id".to_string());
    }
    if call_open
        .get("caller_ephemeral_pub")
        .and_then(Value::as_str)
        != Some(claim_caller_ephemeral.as_str())
    {
        return Err("client call_open did not receive frozen caller_ephemeral_pub".to_string());
    }
    if call_open
        .get("client_ephemeral_pub")
        .and_then(Value::as_str)
        != Some(client_ephemeral_pub.as_str())
    {
        return Err("client call_open did not receive client_ephemeral_pub".to_string());
    }

    let mut caller_events = open_sse_auth(
        &http,
        &format!("{base_url}/v4/remote-invoke/calls/{call_id}/events"),
        &relay_token,
    )
    .await?;
    let _ = next_sse_event(&mut caller_events, Some("connected")).await?;
    post_ok(
        &http,
        &format!("{base_url}/v4/remote-invoke/client/calls/{call_id}/exit"),
        json!({
            "client_instance_id": client_instance_id,
            "exit_code": 0,
            "exit_encrypted": {
                "version": 2,
                "nonce": "ri-pop-exit-nonce",
                "ciphertext": "ri-pop-exit",
                "tag": "ri-pop-exit-tag"
            }
        }),
        Some(&client_auth_token),
    )
    .await?;
    let exit = next_sse_event(&mut caller_events, Some("exit")).await?;
    if exit.get("exit_code").and_then(Value::as_i64) != Some(0) {
        return Err(format!("unexpected exit event: {exit}"));
    }

    post_ok_auth(
        &http,
        &format!("{base_url}/v5/remote-invoke/grants/revoke"),
        sign_pop_body(
            json!({ "client_instance_id": client_instance_id }),
            &caller_keys,
        )?,
        &grant_session_token,
    )
    .await?;

    let revoked_open_status = post_status_auth(
        &http,
        &format!("{base_url}/v5/remote-invoke/calls/open"),
        sign_pop_body(
            json!({
                "client_instance_id": client_instance_id,
                "command_kind": "shell.exec",
                "command_encrypted": {
                    "version": 2,
                    "nonce": "ri-pop-open-after-revoke",
                    "ciphertext": "ri-pop-command",
                    "tag": "ri-pop-tag"
                },
                "command_summary": { "command_preview": "echo after revoke" }
            }),
            &caller_keys,
        )?,
        &grant_session_token,
    )
    .await?;
    if revoked_open_status != StatusCode::UNAUTHORIZED {
        return Err(format!(
            "expected open after revoke to return 401, got {revoked_open_status}"
        ));
    }

    Ok(())
}

async fn start_sync_server() -> Result<(SyncServerGuard, String), String> {
    let port = pick_unused_port().ok_or_else(|| "no unused port available".to_string())?;
    let data_dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| "cannot resolve repo root".to_string())?
        .to_path_buf();

    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/C", "pnpm"]);
        command
    } else {
        Command::new("pnpm")
    };
    let mut child = command
        .current_dir(&repo_root)
        .args([
            "-C",
            "packages/bifrost-sync-server",
            "exec",
            "tsx",
            "src/cli.ts",
            "-p",
            &port.to_string(),
            "-H",
            "127.0.0.1",
            "-d",
            data_dir
                .path()
                .to_str()
                .ok_or_else(|| "temp data dir is not utf-8".to_string())?,
            "--enable-remote-invoke",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn sync-server: {e}"))?;

    let addr = format!("127.0.0.1:{port}");
    let ready = timeout(Duration::from_secs(20), async {
        loop {
            if TcpStream::connect(&addr).is_ok() {
                break;
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("sync-server exited early with {status}"));
            }
            sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    })
    .await
    .map_err(|_| "timed out waiting for sync-server port".to_string())?;
    ready?;

    Ok((
        SyncServerGuard {
            child,
            _data_dir: data_dir,
        },
        format!("http://127.0.0.1:{port}"),
    ))
}

async fn register_user(http: &Client, base_url: &str, user_id: &str) -> Result<String, String> {
    let body = post_ok(
        http,
        &format!("{base_url}/v4/sso/register"),
        json!({
            "user_id": user_id,
            "password": "password123"
        }),
        None,
    )
    .await?;
    data_str(&body, "token").map(str::to_string)
}

async fn register_client(
    http: &Client,
    base_url: &str,
    client_instance_id: &str,
    client_pubkey: &str,
    client_keys: &Ed25519KeyPair,
    user_token: &str,
) -> Result<String, String> {
    let challenge = post_ok(
        http,
        &format!("{base_url}/v4/remote-invoke/client/register/challenge"),
        json!({ "client_instance_id": client_instance_id }),
        Some(user_token),
    )
    .await?;
    let challenge_id = data_str(&challenge, "challenge_id")?;
    let challenge_value = data_str(&challenge, "challenge")?;
    let timestamp = (now_millis() / 1000) as u64;
    let payload = json!([
        "bifrost-remote-register-v1",
        challenge_id,
        challenge_value,
        client_instance_id,
        "ri-pop-client",
        "macos",
        "0.0.0-e2e",
        client_pubkey,
        timestamp,
    ])
    .to_string();
    let signature =
        base64::engine::general_purpose::STANDARD.encode(client_keys.sign(payload.as_bytes()));

    let body = post_ok(
        http,
        &format!("{base_url}/v4/remote-invoke/client/register"),
        json!({
            "challenge_id": challenge_id,
            "client_instance_id": client_instance_id,
            "client_long_term_pubkey": client_pubkey,
            "device_name": "ri-pop-client",
            "platform": "macos",
            "bifrost_version": "0.0.0-e2e",
            "signature": signature,
            "timestamp": timestamp,
        }),
        Some(user_token),
    )
    .await?;
    data_str(&body, "client_auth_token").map(str::to_string)
}

async fn post_ok(
    http: &Client,
    url: &str,
    body: Value,
    bearer: Option<&str>,
) -> Result<Value, String> {
    let mut req = http.post(url).json(&body);
    if let Some(token) = bearer {
        req = req.bearer_auth(token).header("x-bifrost-token", token);
    }
    let response = req.send().await.map_err(|e| e.to_string())?;
    parse_ok_response(response, url).await
}

async fn post_ok_auth(
    http: &Client,
    url: &str,
    body: Value,
    bearer: &str,
) -> Result<Value, String> {
    let response = http
        .post(url)
        .bearer_auth(bearer)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    parse_ok_response(response, url).await
}

async fn post_status_auth(
    http: &Client,
    url: &str,
    body: Value,
    bearer: &str,
) -> Result<StatusCode, String> {
    let response = http
        .post(url)
        .bearer_auth(bearer)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(response.status())
}

async fn parse_ok_response(response: Response, label: &str) -> Result<Value, String> {
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("{label} returned {status}: {text}"));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("parse {label}: {e}; {text}"))?;
    if value.get("code").and_then(Value::as_i64) != Some(0) {
        return Err(format!("{label} returned nonzero code: {value}"));
    }
    Ok(value)
}

async fn open_sse(http: &Client, url: &str) -> Result<Response, String> {
    let response = http.get(url).send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("open SSE {url} returned {status}: {text}"));
    }
    Ok(response)
}

async fn open_sse_auth(http: &Client, url: &str, bearer: &str) -> Result<Response, String> {
    let response = http
        .get(url)
        .bearer_auth(bearer)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("open SSE {url} returned {status}: {text}"));
    }
    Ok(response)
}

async fn next_sse_event(response: &mut Response, expected: Option<&str>) -> Result<Value, String> {
    let mut buffer = String::new();
    let deadline = Duration::from_secs(10);
    timeout(deadline, async {
        loop {
            let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? else {
                return Err("SSE stream closed".to_string());
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(boundary) = buffer.find("\n\n") {
                let raw = buffer[..boundary].to_string();
                buffer = buffer[boundary + 2..].to_string();
                let mut event = "message".to_string();
                let mut data_lines = Vec::new();
                for line in raw.lines() {
                    if let Some(value) = line.strip_prefix("event:") {
                        event = value.trim().to_string();
                    } else if let Some(value) = line.strip_prefix("data:") {
                        data_lines.push(value.trim().to_string());
                    }
                }
                if data_lines.is_empty() {
                    continue;
                }
                if expected.is_some_and(|expected| expected != event) {
                    continue;
                }
                let raw_data = data_lines.join("\n");
                return serde_json::from_str(&raw_data)
                    .map_err(|e| format!("parse SSE {event}: {e}; {raw_data}"));
            }
        }
    })
    .await
    .map_err(|_| format!("timed out waiting for SSE event {:?}", expected))?
}

fn data_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get("data")
        .and_then(|data| data.get(key))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("response data missing string field {key}: {value}"))
}

fn assert_no_approved_crypto_context(value: &Value) -> Result<(), String> {
    for field in ["grant_id", "caller_ephemeral_pub", "client_ephemeral_pub"] {
        if value.get(field).is_some() {
            return Err(format!("approved SSE leaked {field}: {value}"));
        }
    }
    if value
        .get("grant_summary")
        .and_then(|summary| summary.get("client_ephemeral_pub"))
        .is_some()
    {
        return Err(format!(
            "approved SSE grant_summary leaked client_ephemeral_pub: {value}"
        ));
    }
    Ok(())
}

fn generate_ed25519_keypair() -> Result<Ed25519KeyPair, String> {
    let rng = SystemRandom::new();
    let pkcs8 =
        Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "generate ed25519 key".to_string())?;
    Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| "parse ed25519 key".to_string())
}

fn ed25519_pubkey_b64(key_pair: &Ed25519KeyPair) -> String {
    let mut der =
        Vec::with_capacity(ED25519_SPKI_PREFIX.len() + key_pair.public_key().as_ref().len());
    der.extend_from_slice(&ED25519_SPKI_PREFIX);
    der.extend_from_slice(key_pair.public_key().as_ref());
    base64::engine::general_purpose::STANDARD.encode(der)
}

fn sign_pop_body(mut body: Value, key_pair: &Ed25519KeyPair) -> Result<Value, String> {
    {
        let object = body
            .as_object_mut()
            .ok_or_else(|| "PoP body must be an object".to_string())?;
        object.insert("ts".to_string(), Value::from(now_millis() as u64));
        object.insert("nonce".to_string(), Value::String(random_nonce_hex()?));
        object.insert(
            "caller_pubkey".to_string(),
            Value::String(ed25519_pubkey_b64(key_pair)),
        );
        object.remove("signature");
    }
    let canonical = canonical_json(&body)?;
    body.as_object_mut().expect("PoP body object").insert(
        "signature".to_string(),
        Value::String(
            base64::engine::general_purpose::STANDARD.encode(key_pair.sign(canonical.as_bytes())),
        ),
    );
    Ok(body)
}

fn canonical_json(value: &Value) -> Result<String, String> {
    serde_json::to_string(&canonicalize(value)).map_err(|e| e.to_string())
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        Value::Object(map) => {
            let mut sorted = BTreeMap::new();
            for (key, value) in map {
                if key != "signature" {
                    sorted.insert(key.clone(), canonicalize(value));
                }
            }
            let mut out = Map::new();
            for (key, value) in sorted {
                out.insert(key, value);
            }
            Value::Object(out)
        }
        _ => value.clone(),
    }
}

fn random_x25519_pub_b64() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "generate x25519 pub bytes".to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn random_nonce_hex() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "generate nonce".to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
