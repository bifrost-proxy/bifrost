//! Chunked large-file transfer orchestration (caller side, Phase 4 + 4.1).
//!
//! `bifrost remote file upload/download` cannot use the single-shot
//! build/open_call path because a file may be many times larger than the Relay
//! per-frame body limit. Instead we drive a sequence of independent `file.*`
//! remote-invoke calls:
//!
//!   upload:   upload_begin -> upload_chunk* -> upload_commit  (upload_abort on
//!             fatal error; upload_status/begin.received_offset for resume)
//!   download: download_begin -> download_chunk*               (until total_size)
//!
//! Each chunk carries a per-chunk sha256 the server verifies over the *raw*
//! (decoded) bytes before touching disk; the whole file is verified end-to-end
//! via sha256 on both sides. Interrupted transfers resume from the byte offset
//! the server already holds (`.part` size for upload, local `.part` size for
//! download), so no bytes are re-sent needlessly.
//!
//! ## Throughput / packet-size optimizations (Phase 4.1)
//!
//! The Relay is a content-agnostic dumb pipe, so the CLI owns encoding + pacing:
//!
//! * **Pipelining** — instead of one blocking round-trip per chunk, we keep a
//!   bounded window of chunk calls in flight (`UPLOAD_WINDOW`/`DOWNLOAD_WINDOW`)
//!   so relay RTT is amortized across the window. Chunks are offset-addressed
//!   and independent; the server lands upload chunks to the `.part` file in
//!   offset order via a bounded reorder buffer, and the downloader reorders
//!   completed chunks locally before writing, so the on-disk `.part` stays a
//!   contiguous prefix and `--resume` semantics are unchanged.
//! * **Adaptive per-chunk zstd** — when the peer advertises `zstd` we compress
//!   a chunk only if it actually shrinks (already-compressed data self-falls
//!   back to `none`), tagged with `chunk_encoding`. The sha256 is always over
//!   the raw bytes, so integrity is independent of the wire encoding.
//! * **Skip-if-identical** — `upload_begin` may return `already_complete` when
//!   the target already holds a byte-identical file; the chunk loop is skipped.
//!
//! See `design/remote-file-transfer.md` for the protocol + budget rationale.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use colored::Colorize;
use futures::stream::{self, StreamExt};
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

/// Number of upload chunk calls kept in flight. Must stay below the server's
/// reorder-buffer bound (`MAX_PENDING_CHUNKS` = 32) so a burst never trips the
/// "reduce the transfer window" precondition; 8 hides relay RTT with ample
/// headroom and bounds caller memory to `UPLOAD_WINDOW * chunk_size`.
const UPLOAD_WINDOW: usize = 8;

/// Number of download chunk calls kept in flight. The caller reorders completed
/// chunks locally, so worst-case buffered memory is `DOWNLOAD_WINDOW * chunk`.
const DOWNLOAD_WINDOW: usize = 8;

/// zstd level for adaptive chunk compression (matches the server default).
const ZSTD_LEVEL: i32 = 3;

/// Encodings this caller can produce (upload) and decode (download).
fn accept_encodings() -> Value {
    json!(["zstd"])
}

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

/// Adaptively compress an outgoing chunk. Returns `(payload, encoding)` where
/// `encoding` is `"zstd"` only when compression actually shrank the data;
/// otherwise the raw bytes are returned with `"none"` so already-compressed
/// content (jpg/mp4/tar.gz) is never inflated.
fn encode_chunk(raw: &[u8], allow_zstd: bool) -> (Vec<u8>, &'static str) {
    if allow_zstd {
        if let Ok(compressed) = zstd::bulk::compress(raw, ZSTD_LEVEL) {
            if compressed.len() < raw.len() {
                return (compressed, "zstd");
            }
        }
    }
    (raw.to_vec(), "none")
}

/// Decode an incoming chunk payload according to its `chunk_encoding` tag.
/// `cap` bounds the decompressed size (defends against a decompression bomb):
/// a raw chunk can never exceed the negotiated chunk size.
fn decode_chunk(payload: &[u8], encoding: Option<&str>, cap: usize) -> Result<Vec<u8>> {
    match encoding.unwrap_or("none") {
        "none" => Ok(payload.to_vec()),
        "zstd" => zstd::bulk::decompress(payload, cap)
            .map_err(|e| config_err(format!("zstd decompress failed: {}", e))),
        other => Err(config_err(format!(
            "unknown chunk_encoding from server: {}",
            other
        ))),
    }
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

fn upload_begin_args(
    remote: &str,
    cwd: &Option<String>,
    total_size: u64,
    total_sha256: &str,
    chunk_size: Option<u64>,
    overwrite: bool,
    create_parents: bool,
) -> Value {
    json!({
        "path": remote,
        "cwd": cwd,
        "total_size": total_size,
        "total_sha256": total_sha256,
        "chunk_size": chunk_size,
        "allow_overwrite": overwrite,
        "create_parents": create_parents,
        "accept_encodings": accept_encodings(),
    })
}

fn download_begin_args(remote: &str, cwd: Option<String>, chunk_size: Option<u64>) -> Value {
    json!({
        "path": remote,
        "cwd": cwd,
        "chunk_size": chunk_size,
        "accept_encodings": accept_encodings(),
    })
}

fn download_chunk_args(download_id: &str, offset: u64, length: u64) -> Value {
    json!({
        "transfer_id": download_id,
        "chunk_offset": offset,
        "length": length,
    })
}

/// Offsets that partition `[start, total)` into `chunk`-sized spans.
fn chunk_offsets(start: u64, total: u64, chunk: u64) -> Vec<u64> {
    let mut offsets = Vec::new();
    let mut o = start;
    while o < total {
        offsets.push(o);
        o = o.saturating_add(chunk);
    }
    offsets
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
        upload_begin_args(
            remote,
            &cwd,
            total_size,
            &total_sha256,
            chunk_size,
            overwrite,
            create_parents,
        ),
        true,
    )
    .await?;

    // Skip-if-identical: the target already holds byte-identical content, so
    // there is nothing to send. A single round-trip, zero chunks.
    if begin
        .get("already_complete")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        finish_upload_report(output, remote, total_size, &total_sha256, true);
        return Ok(());
    }

    let upload_id = str_field(&begin, "upload_id")?.to_string();
    let chunk = u64_field(&begin, "effective_chunk_size")?.max(1);
    let allow_zstd = begin
        .get("chunk_encoding")
        .and_then(Value::as_str)
        .map(|e| e.eq_ignore_ascii_case("zstd"))
        .unwrap_or(false);
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
    let (active_id, start_offset, allow_zstd) = if resume && server_offset > 0 {
        (upload_id, server_offset, allow_zstd)
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
            upload_begin_args(
                remote,
                &cwd,
                total_size,
                &total_sha256,
                chunk_size,
                overwrite,
                create_parents,
            ),
            true,
        )
        .await?;
        if rebegin
            .get("already_complete")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            finish_upload_report(output, remote, total_size, &total_sha256, true);
            return Ok(());
        }
        let rezstd = rebegin
            .get("chunk_encoding")
            .and_then(Value::as_str)
            .map(|e| e.eq_ignore_ascii_case("zstd"))
            .unwrap_or(false);
        (str_field(&rebegin, "upload_id")?.to_string(), 0, rezstd)
    } else {
        (upload_id, 0, allow_zstd)
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
        allow_zstd,
        no_progress,
        output,
    )
    .await
}

/// Read one chunk from `local_path` at `offset` (independent file handle so the
/// read can run concurrently with other in-flight chunks).
async fn read_chunk_at(local_path: &Path, offset: u64, want: usize) -> Result<Vec<u8>> {
    let mut f = fs::File::open(local_path)
        .await
        .map_err(|e| config_err(format!("open {}: {}", local_path.display(), e)))?;
    f.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| config_err(format!("seek {}: {}", local_path.display(), e)))?;
    let mut buf = vec![0u8; want];
    f.read_exact(&mut buf)
        .await
        .map_err(|e| config_err(format!("read {}: {}", local_path.display(), e)))?;
    Ok(buf)
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
    allow_zstd: bool,
    no_progress: bool,
    output: &str,
) -> Result<()> {
    let offsets = chunk_offsets(start_offset, total_size, chunk);
    let done = AtomicU64::new(start_offset);
    print_progress(no_progress, "uploading", start_offset, total_size);

    // Pipeline: keep UPLOAD_WINDOW chunk calls in flight. Each chunk is
    // offset-addressed and independent; the server lands them in offset order
    // via its bounded reorder buffer, so ordering of completion is irrelevant
    // to on-disk integrity.
    let mut in_flight = stream::iter(offsets.into_iter().map(|offset| {
        let want = ((total_size - offset).min(chunk)) as usize;
        async move {
            let raw = read_chunk_at(local_path, offset, want).await?;
            let chunk_sha = sha256_hex(&raw);
            let (payload, encoding) = encode_chunk(&raw, allow_zstd);
            run_op(
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
                    "chunk_b64": b64_encode(&payload),
                    "chunk_sha256": chunk_sha,
                    "chunk_encoding": encoding,
                }),
                true,
            )
            .await
            .map(|_| want as u64)
        }
    }))
    .buffer_unordered(UPLOAD_WINDOW);

    while let Some(res) = in_flight.next().await {
        let n = res?;
        let now = done.fetch_add(n, Ordering::Relaxed) + n;
        print_progress(no_progress, "uploading", now, total_size);
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
    finish_upload_report(output, remote_path, bytes, remote_sha, false);
    Ok(())
}

fn finish_upload_report(output: &str, path: &str, bytes: u64, sha256: &str, skipped: bool) {
    if output.eq_ignore_ascii_case("json") {
        println!(
            "{}",
            json!({
                "status": "ok",
                "operation": "upload",
                "path": path,
                "bytes_written": bytes,
                "sha256": sha256,
                "skipped": skipped,
            })
        );
    } else if skipped {
        eprintln!(
            "  {} {} already up to date ({} bytes, sha256 {}) — nothing to send",
            "✓".bright_green(),
            path,
            bytes,
            &sha256[..sha256.len().min(16)]
        );
    } else {
        eprintln!(
            "  {} uploaded {} ({} bytes, sha256 {}) verified",
            "✓".bright_green(),
            path,
            bytes,
            &sha256[..sha256.len().min(16)]
        );
    }
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
    let start_offset = offset;

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

    // Pipeline: fetch DOWNLOAD_WINDOW chunks concurrently, then land them to
    // the .part strictly in offset order via a local reorder buffer so the
    // on-disk file stays a contiguous prefix (resume-safe). Completed but
    // not-yet-writable chunks are bounded by the window size.
    let offsets = chunk_offsets(start_offset, total_size, effective_chunk_size);
    let cap = effective_chunk_size as usize;
    let mut fetched = stream::iter(offsets.into_iter().map(|off| {
        let want = (total_size - off).min(effective_chunk_size);
        let download_id = download_id.clone();
        async move {
            let resp = run_op(
                caller,
                conn,
                grant,
                caller_identity,
                transport,
                "file.download_chunk",
                &format!("download chunk @{}", off),
                download_chunk_args(&download_id, off, want),
                true,
            )
            .await?;
            let payload = b64_decode(str_field(&resp, "chunk_b64")?)?;
            let encoding = resp.get("chunk_encoding").and_then(Value::as_str);
            let bytes = decode_chunk(&payload, encoding, cap)?;
            if let Some(expected) = resp.get("chunk_sha256").and_then(Value::as_str) {
                let actual = sha256_hex(&bytes);
                if !actual.eq_ignore_ascii_case(expected) {
                    return Err(config_err(format!(
                        "chunk sha mismatch at offset {}: expected {}, got {}",
                        off, expected, actual
                    )));
                }
            }
            Ok::<(u64, Vec<u8>), BifrostError>((off, bytes))
        }
    }))
    .buffer_unordered(DOWNLOAD_WINDOW);

    let mut pending: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    let mut write_offset = start_offset;
    while let Some(res) = fetched.next().await {
        let (off, bytes) = res?;
        pending.insert(off, bytes);
        // Drain any chunks that are now contiguous with the write frontier.
        while let Some(bytes) = pending.remove(&write_offset) {
            file.write_all(&bytes)
                .await
                .map_err(|e| config_err(format!("write {}: {}", part_path.display(), e)))?;
            write_offset += bytes.len() as u64;
            print_progress(no_progress, "downloading", write_offset, total_size);
        }
    }
    file.flush()
        .await
        .map_err(|e| config_err(format!("flush {}: {}", part_path.display(), e)))?;
    drop(file);
    finish_progress(no_progress);

    if write_offset != total_size {
        return Err(config_err(format!(
            "download incomplete: wrote {} of {} bytes (partial kept at {})",
            write_offset,
            total_size,
            part_path.display()
        )));
    }

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
    fn encode_chunk_compresses_and_round_trips() {
        // Highly compressible -> zstd wins.
        let compressible = vec![0u8; 64 * 1024];
        let (payload, enc) = encode_chunk(&compressible, true);
        assert_eq!(enc, "zstd");
        assert!(payload.len() < compressible.len());
        let back = decode_chunk(&payload, Some("zstd"), 64 * 1024).unwrap();
        assert_eq!(back, compressible);

        // Incompressible -> falls back to none, no inflation.
        use ring::rand::{SecureRandom, SystemRandom};
        let mut incompressible = vec![0u8; 4096];
        SystemRandom::new().fill(&mut incompressible).unwrap();
        let (payload2, enc2) = encode_chunk(&incompressible, true);
        assert_eq!(enc2, "none");
        assert_eq!(payload2, incompressible);

        // Opt-out always yields raw / none.
        let (payload3, enc3) = encode_chunk(&compressible, false);
        assert_eq!(enc3, "none");
        assert_eq!(payload3, compressible);
    }

    #[test]
    fn decode_chunk_handles_none_and_rejects_unknown() {
        assert_eq!(decode_chunk(b"abc", Some("none"), 16).unwrap(), b"abc");
        assert_eq!(decode_chunk(b"abc", None, 16).unwrap(), b"abc");
        assert!(decode_chunk(b"abc", Some("lz4"), 16).is_err());
    }

    #[test]
    fn chunk_offsets_partition_range() {
        assert_eq!(chunk_offsets(0, 10, 4), vec![0, 4, 8]);
        assert_eq!(chunk_offsets(4, 10, 4), vec![4, 8]);
        assert_eq!(chunk_offsets(0, 0, 4), Vec::<u64>::new());
        assert_eq!(chunk_offsets(0, 8, 4), vec![0, 4]);
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
    fn transfer_args_negotiate_encoding_and_sizes() {
        let begin = download_begin_args("remote.bin", Some("/repo".to_string()), Some(65_536));
        assert_eq!(begin["path"], "remote.bin");
        assert_eq!(begin["cwd"], "/repo");
        assert_eq!(begin["chunk_size"].as_u64(), Some(65_536));
        assert_eq!(begin["accept_encodings"], json!(["zstd"]));

        let up = upload_begin_args("r.bin", &None, 100, "ab", Some(4096), true, false);
        assert_eq!(up["total_size"].as_u64(), Some(100));
        assert_eq!(up["accept_encodings"], json!(["zstd"]));

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

    #[tokio::test]
    async fn read_chunk_at_reads_requested_span() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.bin");
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 256) as u8).collect();
        tokio::fs::write(&path, &data).await.unwrap();
        let got = read_chunk_at(&path, 100, 50).await.unwrap();
        assert_eq!(got, &data[100..150]);
    }
}
