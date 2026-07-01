//! Chunked large-file transfer orchestration (caller side, Phase 4).
//!
//! `bifrost remote file upload/download` cannot use the single-shot
//! build/open_call path because a file may be many times larger than the 10 MB
//! Relay per-call limit. Instead we drive a sequence of independent `file.*`
//! remote-invoke calls:
//!
//!   upload:   upload_begin -> upload_chunk* -> upload_commit  (upload_abort on
//!             fatal error; upload_status/begin.received_offset for resume)
//!   download: download_begin -> download_chunk*               (until eof)
//!
//! Each chunk is base64-encoded and carries a per-chunk sha256 the server
//! verifies before touching disk; the whole file is verified end-to-end via
//! sha256 on both sides. Interrupted transfers resume from the byte offset the
//! server already holds (`.part` size for upload, local `.part` size for
//! download), so no bytes are re-sent needlessly.
//!
//! See `design/remote-file-transfer.md` for the protocol + budget rationale.

use std::io::Write as _;
use std::path::Path;

use base64::Engine;
use colored::Colorize;
use ring::digest::{Context, SHA256};
use serde_json::{json, Value};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use bifrost_core::{BifrostError, Result};

use super::{
    open_and_wait_remote_command, BuiltRemoteCommand, CallResult, CallerPopIdentity,
    CallerRelayClient, CommandKind, GrantInfo, LocalConnection, OpenCallTransportContext,
    RemoteRenderMode, CALL_EVENT_TIMEOUT_SECS,
};
use crate::cli::RemoteFileCommands;

/// Per-chunk retry budget for transient relay/IO failures before giving up.
const CHUNK_MAX_RETRIES: u32 = 3;

/// Entry point invoked from `async_handle_remote_command` for the
/// `Upload`/`Download` file subcommands.
pub(super) async fn handle_remote_file_transfer(
    caller: &CallerRelayClient,
    conn: &LocalConnection,
    grant: &GrantInfo,
    caller_identity: &CallerPopIdentity,
    transport: &OpenCallTransportContext,
    action: &RemoteFileCommands,
) -> Result<()> {
    match action {
        RemoteFileCommands::Upload {
            local,
            remote,
            chunk_size,
            overwrite,
            create_parents,
            resume,
            no_progress,
            cwd,
            output,
        } => {
            run_upload(
                caller,
                conn,
                grant,
                caller_identity,
                transport,
                local,
                remote,
                *chunk_size,
                *overwrite,
                *create_parents,
                *resume,
                *no_progress,
                cwd.clone(),
                output,
            )
            .await
        }
        RemoteFileCommands::Download {
            remote,
            local,
            chunk_size,
            overwrite,
            resume,
            no_progress,
            cwd,
            output,
        } => {
            run_download(
                caller,
                conn,
                grant,
                caller_identity,
                transport,
                remote,
                local,
                *chunk_size,
                *overwrite,
                *resume,
                *no_progress,
                cwd.clone(),
                output,
            )
            .await
        }
        _ => Err(BifrostError::Config(
            "handle_remote_file_transfer called with a non-transfer action".to_string(),
        )),
    }
}

// ---- small local helpers ----

fn config_err(msg: impl Into<String>) -> BifrostError {
    BifrostError::Config(msg.into())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut ctx = Context::new(&SHA256);
    ctx.update(bytes);
    ctx.finish()
        .as_ref()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

async fn sha256_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path)
        .await
        .map_err(|e| config_err(format!("open {}: {}", path.display(), e)))?;
    let mut ctx = Context::new(&SHA256);
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .await
            .map_err(|e| config_err(format!("read {}: {}", path.display(), e)))?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    Ok(ctx
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect())
}

fn b64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn b64_decode(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .map_err(|e| config_err(format!("invalid base64 chunk from server: {}", e)))
}

/// Build a raw `file.*` command whose response JSON we parse ourselves.
///
/// Uses `RemoteRenderMode::Capture` (NOT `Raw`): `Raw` streams stdout straight
/// to the terminal and leaves `CallResult.stdout` empty, which would make every
/// intermediate transfer op (begin/chunk/commit) look like an "empty response"
/// to the orchestrator. `Capture` buffers stdout into the result and prints
/// nothing, so we can parse each op's JSON programmatically.
fn build_raw_file_command(op: &str, label: String, args: Value) -> BuiltRemoteCommand {
    BuiltRemoteCommand {
        kind: CommandKind::File,
        label,
        command: Some(op.to_string()),
        args_json: Some(args.to_string()),
        query: None,
        shell_exec: None,
        render: RemoteRenderMode::Capture,
        streaming_prefs: None,
    }
}

/// Run one transfer op and return its parsed JSON `data` object. Retries
/// transient failures up to `CHUNK_MAX_RETRIES`; a non-zero exit_code (a
/// server-side policy/validation error) is treated as fatal and not retried.
#[allow(clippy::too_many_arguments)]
async fn run_op(
    caller: &CallerRelayClient,
    conn: &LocalConnection,
    grant: &GrantInfo,
    caller_identity: &CallerPopIdentity,
    transport: &OpenCallTransportContext,
    op: &str,
    label: &str,
    args: Value,
    retryable: bool,
) -> Result<Value> {
    let mut attempt = 0u32;
    loop {
        let command = build_raw_file_command(op, label.to_string(), args.clone());
        let result: std::result::Result<CallResult, BifrostError> = open_and_wait_remote_command(
            caller,
            conn,
            grant,
            caller_identity,
            transport,
            &command,
            CALL_EVENT_TIMEOUT_SECS,
        )
        .await;

        match result {
            Ok(call) if call.exit_code == 0 => {
                let stdout = call.stdout.unwrap_or_default();
                return parse_op_stdout(op, &stdout);
            }
            Ok(call) => {
                // Non-zero exit == server rejected the op (policy, sha, size,
                // precondition). These are deterministic; do not retry.
                let detail = call
                    .stderr
                    .or(call.stdout)
                    .unwrap_or_else(|| format!("exit_code={}", call.exit_code));
                return Err(config_err(format!("{} failed: {}", op, detail.trim())));
            }
            Err(err) => {
                if retryable && attempt < CHUNK_MAX_RETRIES {
                    attempt += 1;
                    continue;
                }
                return Err(err);
            }
        }
    }
}

/// The server serializes the op result as a JSON object into stdout.
fn parse_op_stdout(op: &str, stdout: &str) -> Result<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err(config_err(format!("{} returned an empty response", op)));
    }
    serde_json::from_str::<Value>(trimmed).map_err(|e| {
        config_err(format!(
            "{} returned unparseable JSON: {} ({})",
            op, e, trimmed
        ))
    })
}

fn u64_field(v: &Value, key: &str) -> Result<u64> {
    v.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| config_err(format!("server response missing numeric field '{}'", key)))
}

fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| config_err(format!("server response missing string field '{}'", key)))
}

fn download_begin_args(remote: &str, cwd: Option<String>, chunk_size: Option<u64>) -> Value {
    json!({
        "path": remote,
        "cwd": cwd,
        "chunk_size": chunk_size,
    })
}

fn download_chunk_args(download_id: &str, offset: u64, length: u64) -> Value {
    json!({
        "transfer_id": download_id,
        "chunk_offset": offset,
        "length": length,
    })
}

fn print_progress(no_progress: bool, label: &str, done: u64, total: u64) {
    if no_progress {
        return;
    }
    let pct = if total > 0 {
        (done as f64 / total as f64) * 100.0
    } else {
        100.0
    };
    // Carriage-return in-place update on the same line.
    eprint!(
        "\r  {} {} {:>6.2}%  ({} / {} bytes)",
        "→".bright_cyan(),
        label,
        pct,
        done,
        total
    );
    let _ = std::io::stderr().flush();
}

fn finish_progress(no_progress: bool) {
    if !no_progress {
        eprintln!();
    }
}

// ---- upload ----

#[allow(clippy::too_many_arguments)]
async fn run_upload(
    caller: &CallerRelayClient,
    conn: &LocalConnection,
    grant: &GrantInfo,
    caller_identity: &CallerPopIdentity,
    transport: &OpenCallTransportContext,
    local: &str,
    remote: &str,
    chunk_size: Option<u64>,
    overwrite: bool,
    create_parents: bool,
    resume: bool,
    no_progress: bool,
    cwd: Option<String>,
    output: &str,
) -> Result<()> {
    let local_path = Path::new(local);
    let md = fs::metadata(local_path)
        .await
        .map_err(|e| config_err(format!("cannot read local file {}: {}", local, e)))?;
    if md.is_dir() {
        return Err(config_err(format!(
            "local source is a directory, not a file: {}",
            local
        )));
    }
    let total_size = md.len();
    let total_sha256 = sha256_file(local_path).await?;

    let begin = run_op(
        caller,
        conn,
        grant,
        caller_identity,
        transport,
        "file.upload_begin",
        &format!("upload begin {}", remote),
        json!({
            "path": remote,
            "cwd": cwd,
            "total_size": total_size,
            "total_sha256": total_sha256,
            "chunk_size": chunk_size,
            "allow_overwrite": overwrite,
            "create_parents": create_parents,
        }),
        true,
    )
    .await?;

    let upload_id = str_field(&begin, "upload_id")?.to_string();
    let chunk = u64_field(&begin, "effective_chunk_size")?.max(1);
    // Resume point: the server reports how many bytes of a matching .part it
    // already holds. Honour it only when --resume was requested.
    let server_offset = begin
        .get("received_offset")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(total_size);

    // Resume semantics:
    //   --resume + matching .part on server -> continue from server_offset.
    //   otherwise, if the server is holding a stale .part, abort + re-begin so
    //   we start cleanly at offset 0 (the server's in-order append check would
    //   otherwise reject a chunk at offset 0 against a non-empty .part).
    let (active_id, start_offset) = if resume && server_offset > 0 {
        (upload_id, server_offset)
    } else if server_offset > 0 {
        let _ = run_op(
            caller,
            conn,
            grant,
            caller_identity,
            transport,
            "file.upload_abort",
            "upload abort (stale part)",
            json!({ "transfer_id": upload_id }),
            true,
        )
        .await;
        let rebegin = run_op(
            caller,
            conn,
            grant,
            caller_identity,
            transport,
            "file.upload_begin",
            &format!("upload begin {}", remote),
            json!({
                "path": remote,
                "cwd": cwd,
                "total_size": total_size,
                "total_sha256": total_sha256,
                "chunk_size": chunk_size,
                "allow_overwrite": overwrite,
                "create_parents": create_parents,
            }),
            true,
        )
        .await?;
        (str_field(&rebegin, "upload_id")?.to_string(), 0)
    } else {
        (upload_id, 0)
    };

    upload_chunks_and_commit(
        caller,
        conn,
        grant,
        caller_identity,
        transport,
        local_path,
        remote,
        &active_id,
        chunk,
        start_offset,
        total_size,
        &total_sha256,
        no_progress,
        output,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn upload_chunks_and_commit(
    caller: &CallerRelayClient,
    conn: &LocalConnection,
    grant: &GrantInfo,
    caller_identity: &CallerPopIdentity,
    transport: &OpenCallTransportContext,
    local_path: &Path,
    remote: &str,
    upload_id: &str,
    chunk: u64,
    start_offset: u64,
    total_size: u64,
    total_sha256: &str,
    no_progress: bool,
    output: &str,
) -> Result<()> {
    let mut f = fs::File::open(local_path)
        .await
        .map_err(|e| config_err(format!("open {}: {}", local_path.display(), e)))?;
    let mut offset = start_offset;
    if offset > 0 {
        f.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| config_err(format!("seek {}: {}", local_path.display(), e)))?;
    }
    print_progress(no_progress, "uploading", offset, total_size);

    let mut buf = vec![0u8; chunk as usize];
    while offset < total_size {
        let want = ((total_size - offset).min(chunk)) as usize;
        f.read_exact(&mut buf[..want])
            .await
            .map_err(|e| config_err(format!("read {}: {}", local_path.display(), e)))?;
        let slice = &buf[..want];
        let chunk_sha = sha256_hex(slice);
        let chunk_b64 = b64_encode(slice);

        let resp = run_op(
            caller,
            conn,
            grant,
            caller_identity,
            transport,
            "file.upload_chunk",
            &format!("upload chunk @{}", offset),
            json!({
                "transfer_id": upload_id,
                "chunk_offset": offset,
                "chunk_b64": chunk_b64,
                "chunk_sha256": chunk_sha,
            }),
            true,
        )
        .await?;
        let next = u64_field(&resp, "next_offset")?;
        if next <= offset {
            return Err(config_err(format!(
                "server did not advance offset (was {}, got {})",
                offset, next
            )));
        }
        offset = next;
        print_progress(no_progress, "uploading", offset, total_size);
    }
    finish_progress(no_progress);

    let commit = run_op(
        caller,
        conn,
        grant,
        caller_identity,
        transport,
        "file.upload_commit",
        &format!("upload commit {}", remote),
        json!({
            "transfer_id": upload_id,
            "total_sha256": total_sha256,
        }),
        true,
    )
    .await?;

    let remote_sha = str_field(&commit, "sha256")?;
    if !remote_sha.eq_ignore_ascii_case(total_sha256) {
        return Err(config_err(format!(
            "integrity check failed: local sha256 {} != remote sha256 {}",
            total_sha256, remote_sha
        )));
    }
    let bytes = commit
        .get("bytes_written")
        .and_then(Value::as_u64)
        .unwrap_or(total_size);
    let remote_path = commit.get("path").and_then(Value::as_str).unwrap_or(remote);

    if output.eq_ignore_ascii_case("json") {
        println!(
            "{}",
            json!({
                "status": "ok",
                "operation": "upload",
                "path": remote_path,
                "bytes_written": bytes,
                "sha256": remote_sha,
            })
        );
    } else {
        eprintln!(
            "  {} uploaded {} ({} bytes, sha256 {}) verified",
            "✓".bright_green(),
            remote_path,
            bytes,
            &remote_sha[..remote_sha.len().min(16)]
        );
    }
    Ok(())
}

// ---- download ----

#[allow(clippy::too_many_arguments)]
async fn run_download(
    caller: &CallerRelayClient,
    conn: &LocalConnection,
    grant: &GrantInfo,
    caller_identity: &CallerPopIdentity,
    transport: &OpenCallTransportContext,
    remote: &str,
    local: &str,
    chunk_size: Option<u64>,
    overwrite: bool,
    resume: bool,
    no_progress: bool,
    cwd: Option<String>,
    output: &str,
) -> Result<()> {
    let final_path = Path::new(local);
    if fs::try_exists(final_path).await.unwrap_or(false) && !overwrite && !resume {
        return Err(config_err(format!(
            "local destination already exists and --overwrite was not passed: {}",
            local
        )));
    }
    if let Some(parent) = final_path.parent() {
        if !parent.as_os_str().is_empty() && !fs::try_exists(parent).await.unwrap_or(false) {
            return Err(config_err(format!(
                "local destination directory does not exist: {}",
                parent.display()
            )));
        }
    }

    let begin = run_op(
        caller,
        conn,
        grant,
        caller_identity,
        transport,
        "file.download_begin",
        &format!("download begin {}", remote),
        download_begin_args(remote, cwd, chunk_size),
        true,
    )
    .await?;
    let download_id = str_field(&begin, "download_id")?.to_string();
    let total_size = u64_field(&begin, "total_size")?;
    let total_sha256 = str_field(&begin, "total_sha256")?.to_string();
    let effective_chunk_size = u64_field(&begin, "effective_chunk_size")?.max(1);

    // Write to a sibling .part; rename on success for atomicity + resume.
    let part_path = final_path.with_extension(format!(
        "{}bifrost-download.part",
        final_path
            .extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));

    let mut offset = 0u64;
    if resume {
        if let Ok(md) = fs::metadata(&part_path).await {
            offset = md.len().min(total_size);
        }
    } else {
        let _ = fs::remove_file(&part_path).await;
    }

    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(offset == 0)
        .open(&part_path)
        .await
        .map_err(|e| config_err(format!("open {}: {}", part_path.display(), e)))?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| config_err(format!("seek {}: {}", part_path.display(), e)))?;

    print_progress(no_progress, "downloading", offset, total_size);
    while offset < total_size {
        let resp = run_op(
            caller,
            conn,
            grant,
            caller_identity,
            transport,
            "file.download_chunk",
            &format!("download chunk @{}", offset),
            download_chunk_args(&download_id, offset, effective_chunk_size),
            true,
        )
        .await?;
        let chunk_b64 = str_field(&resp, "chunk_b64")?;
        let bytes = b64_decode(chunk_b64)?;
        if let Some(expected) = resp.get("chunk_sha256").and_then(Value::as_str) {
            let actual = sha256_hex(&bytes);
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(config_err(format!(
                    "chunk sha mismatch at offset {}: expected {}, got {}",
                    offset, expected, actual
                )));
            }
        }
        file.write_all(&bytes)
            .await
            .map_err(|e| config_err(format!("write {}: {}", part_path.display(), e)))?;
        let next = u64_field(&resp, "next_offset")?;
        if next <= offset && !bytes.is_empty() {
            return Err(config_err(format!(
                "server did not advance offset (was {}, got {})",
                offset, next
            )));
        }
        offset = next;
        print_progress(no_progress, "downloading", offset, total_size);
        if resp.get("eof").and_then(Value::as_bool).unwrap_or(false) {
            break;
        }
    }
    file.flush()
        .await
        .map_err(|e| config_err(format!("flush {}: {}", part_path.display(), e)))?;
    drop(file);
    finish_progress(no_progress);

    // End-to-end integrity before publishing the final path.
    let actual_sha = sha256_file(&part_path).await?;
    if !actual_sha.eq_ignore_ascii_case(&total_sha256) {
        return Err(config_err(format!(
            "integrity check failed: remote sha256 {} != downloaded sha256 {} (partial kept at {})",
            total_sha256,
            actual_sha,
            part_path.display()
        )));
    }
    fs::rename(&part_path, final_path).await.map_err(|e| {
        config_err(format!(
            "rename {} -> {}: {}",
            part_path.display(),
            final_path.display(),
            e
        ))
    })?;

    if output.eq_ignore_ascii_case("json") {
        println!(
            "{}",
            json!({
                "status": "ok",
                "operation": "download",
                "path": final_path.to_string_lossy(),
                "bytes_written": total_size,
                "sha256": actual_sha,
            })
        );
    } else {
        eprintln!(
            "  {} downloaded {} ({} bytes, sha256 {}) verified",
            "✓".bright_green(),
            final_path.display(),
            total_size,
            &actual_sha[..actual_sha.len().min(16)]
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_matches_known_vector() {
        // sha256("") = e3b0c442...
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // sha256("abc")
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn base64_round_trips() {
        let data = b"the quick brown fox\x00\x01\x02".to_vec();
        let encoded = b64_encode(&data);
        let decoded = b64_decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn b64_decode_rejects_garbage() {
        assert!(b64_decode("not valid base64 !!!").is_err());
    }

    #[test]
    fn parse_op_stdout_parses_object_and_rejects_empty() {
        let v =
            parse_op_stdout("file.upload_begin", "  {\"upload_id\":\"abc\",\"n\":7}  ").unwrap();
        assert_eq!(v["upload_id"], "abc");
        assert_eq!(v["n"], 7);
        assert!(parse_op_stdout("file.upload_begin", "   ").is_err());
        assert!(parse_op_stdout("file.upload_begin", "not json").is_err());
    }

    #[test]
    fn field_extractors_report_missing_and_wrong_type() {
        let v = json!({ "size": 42u64, "name": "x" });
        assert_eq!(u64_field(&v, "size").unwrap(), 42);
        assert_eq!(str_field(&v, "name").unwrap(), "x");
        assert!(u64_field(&v, "missing").is_err());
        assert!(str_field(&v, "missing").is_err());
        // Wrong type is treated as missing.
        assert!(u64_field(&v, "name").is_err());
        assert!(str_field(&v, "size").is_err());
    }

    #[test]
    fn build_raw_file_command_shape() {
        let cmd = build_raw_file_command(
            "file.upload_chunk",
            "chunk".to_string(),
            json!({ "transfer_id": "id", "chunk_offset": 0 }),
        );
        assert!(matches!(cmd.kind, CommandKind::File));
        assert_eq!(cmd.command.as_deref(), Some("file.upload_chunk"));
        assert!(matches!(cmd.render, RemoteRenderMode::Capture));
        let args: Value = serde_json::from_str(cmd.args_json.as_deref().unwrap()).unwrap();
        assert_eq!(args["transfer_id"], "id");
        assert_eq!(args["chunk_offset"], 0);
    }

    #[test]
    fn download_transfer_args_include_requested_and_effective_chunk_sizes() {
        let begin = download_begin_args("remote.bin", Some("/repo".to_string()), Some(65_536));
        assert_eq!(begin["path"], "remote.bin");
        assert_eq!(begin["cwd"], "/repo");
        assert_eq!(begin["chunk_size"].as_u64(), Some(65_536));

        let chunk = download_chunk_args("download-1", 131_072, 65_536);
        assert_eq!(chunk["transfer_id"], "download-1");
        assert_eq!(chunk["chunk_offset"].as_u64(), Some(131_072));
        assert_eq!(chunk["length"].as_u64(), Some(65_536));
    }

    #[tokio::test]
    async fn sha256_file_matches_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.bin");
        let data = b"chunk transfer integrity".repeat(4096);
        tokio::fs::write(&path, &data).await.unwrap();
        assert_eq!(sha256_file(&path).await.unwrap(), sha256_hex(&data));
    }
}
