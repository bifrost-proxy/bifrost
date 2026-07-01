//! Chunked large-file transfer for the Remote Invoke subsystem (Phase 4).
//!
//! The single-shot `file.read` / `file.write` handlers cap out at the Relay
//! per-call limit (10 MB) minus base64 (4/3) and POP-envelope overhead. This
//! module implements resumable, chunked upload (caller -> remote) and download
//! (remote -> caller) so arbitrarily large files can be moved in independent
//! sub-10MB remote-invoke calls, with per-chunk and whole-file sha256
//! integrity.
//!
//! Access control: `file.upload_begin` / `file.download_begin` run through the
//! normal [`bifrost_core::file_access::FileAccessPolicy::check`] pipeline in
//! the executor (op `Upload` / `Download`). Subsequent chunk/commit/abort/
//! status ops operate on an in-memory session keyed by a random id; the
//! canonical, root-confined target path is captured at `begin` time and never
//! re-derived from caller input, so a session cannot be steered outside the
//! policy roots after it is opened.
//!
//! See `design/remote-file-transfer.md` for the protocol and budget rationale.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use bifrost_core::file_access::{FileAccessPolicy, PolicyDecision};
use bifrost_core::{BifrostError, Result};
use ring::digest::{Context, SHA256};
use serde_json::{json, Value};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Idle timeout after which a transfer session is eligible for lazy cleanup.
const SESSION_TTL: Duration = Duration::from_secs(30 * 60);

/// Default chunk size (raw bytes) the server advertises when a caller does not
/// request one.
///
/// The binding constraint is the Relay's per-frame POST body limit
/// (`MAX_BODY_SIZE` = 2 MiB in bifrost-sync-server). Download chunk bytes travel
/// back to the caller as an encrypted call frame, so the raw chunk is inflated
/// twice by base64 (once for `chunk_b64`, once when the whole stdout JSON is
/// sealed into the POP envelope) plus JSON overhead — roughly a 2x blow-up.
/// A 512 KiB raw chunk therefore lands near ~1 MiB on the wire, comfortably
/// under the 2 MiB frame ceiling with headroom for the envelope.
const DEFAULT_CHUNK_SIZE: u64 = 512 * 1024;

/// Direction of a transfer session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransferKind {
    Upload,
    Download,
}

/// In-memory state for one active transfer. Uploads write to a sibling
/// `.part` file for atomic commit + resume; downloads snapshot the source
/// size + whole-file sha at begin for a consistent view.
struct TransferSession {
    kind: TransferKind,
    /// Canonical target (upload) or source (download) path.
    final_path: PathBuf,
    /// `<dir>/.bifrost-upload.<id>.part` (upload only).
    part_path: Option<PathBuf>,
    total_size: u64,
    total_sha256: String,
    /// Effective (clamped) chunk size advertised to the caller.
    chunk_size: u64,
    /// Unix mode to restore on the committed file (upload only).
    prior_mode: Option<u32>,
    last_activity: Instant,
}

type SessionMap = HashMap<String, TransferSession>;

fn sessions() -> &'static Mutex<SessionMap> {
    static SESSIONS: OnceLock<Mutex<SessionMap>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Drop sessions that have been idle past the TTL. Called opportunistically on
/// every session-map access so a crashed/abandoned client cannot leak state
/// (and its `.part` file remains on disk for a later `--resume`).
fn evict_expired(map: &mut SessionMap) {
    let now = Instant::now();
    map.retain(|_, s| now.duration_since(s.last_activity) < SESSION_TTL);
}

// ---- small self-contained helpers (kept local to avoid widening the
// visibility of the 5000-line file_ops.rs) ----

fn invalid_args(msg: impl Into<String>) -> BifrostError {
    BifrostError::Config(format!("[file.invalid_args] {}", msg.into()))
}
fn precondition_failed(msg: impl Into<String>) -> BifrostError {
    BifrostError::Config(format!("[file.precondition_failed] {}", msg.into()))
}
fn sha_mismatch(msg: impl Into<String>) -> BifrostError {
    BifrostError::Config(format!("[file.sha_mismatch] {}", msg.into()))
}
fn size_too_large(msg: impl Into<String>) -> BifrostError {
    BifrostError::Config(format!("[file.size_too_large] {}", msg.into()))
}
fn not_found(msg: impl Into<String>) -> BifrostError {
    BifrostError::Config(format!("[file.not_found] {}", msg.into()))
}
fn io_err(ctx: &str, err: std::io::Error) -> BifrostError {
    BifrostError::Config(format!("[file.io_error] {}: {}", ctx, err))
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
        .map_err(|e| io_err(&format!("open {}", path.display()), e))?;
    let mut ctx = Context::new(&SHA256);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .await
            .map_err(|e| io_err(&format!("read {}", path.display()), e))?;
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

fn b64_decode(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .map_err(|e| invalid_args(format!("invalid base64 chunk: {}", e)))
}

fn b64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[allow(unused_variables)]
async fn capture_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(path).await {
            Ok(md) => Some(md.permissions().mode() & 0o7777),
            Err(_) => None,
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[allow(unused_variables)]
async fn apply_mode(path: &Path, mode: Option<u32>) {
    #[cfg(unix)]
    {
        if let Some(m) = mode {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, std::fs::Permissions::from_mode(m)).await;
        }
    }
}

/// 128-bit random hex id (unpredictable, so sessions cannot be guessed and
/// hijacked by another caller sharing the same relay).
fn random_id() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let rng = SystemRandom::new();
    let mut buf = [0u8; 16];
    // SystemRandom::fill only fails on catastrophic RNG unavailability; fall
    // back to a time-seeded value so we never panic in a request path.
    if rng.fill(&mut buf).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        buf[..16].copy_from_slice(&nanos.to_le_bytes());
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Params carried by the session-scoped transfer ops (everything except
/// `begin`, which resolves a fresh policy decision).
pub(crate) struct TransferOpParams {
    pub transfer_id: Option<String>,
    pub offset: Option<u64>,
    pub length: Option<u64>,
    pub chunk_b64: Option<String>,
    pub chunk_sha256: Option<String>,
    pub total_sha256: Option<String>,
}

/// Entry point for `file.upload_begin`.
pub(crate) async fn handle_upload_begin(
    decision: &PolicyDecision,
    policy: &FileAccessPolicy,
    total_size: Option<u64>,
    total_sha256: Option<&str>,
    requested_chunk_size: Option<u64>,
    allow_overwrite: Option<bool>,
    create_parents: bool,
) -> Result<Value> {
    let total_size =
        total_size.ok_or_else(|| invalid_args("'total_size' is required for file.upload_begin"))?;
    let total_sha256 = total_sha256
        .ok_or_else(|| invalid_args("'total_sha256' is required for file.upload_begin"))?
        .to_string();

    if total_size > policy.max_transfer_bytes {
        return Err(size_too_large(format!(
            "total_size {} exceeds max_transfer_bytes {}",
            total_size, policy.max_transfer_bytes
        )));
    }

    let final_path = decision.path.as_path().to_path_buf();
    let overwrite = allow_overwrite.unwrap_or(decision.allow_overwrite);

    // Determine the target directory for the .part sibling.
    let parent = final_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    if create_parents {
        fs::create_dir_all(&parent)
            .await
            .map_err(|e| io_err(&format!("create_dir_all {}", parent.display()), e))?;
    }
    if !fs::try_exists(&parent).await.unwrap_or(false) {
        return Err(not_found(format!(
            "parent directory does not exist: {} (pass create_parents=true)",
            parent.display()
        )));
    }

    // Overwrite gating: refuse an existing target when overwrite is off.
    if fs::try_exists(&final_path).await.unwrap_or(false) && !overwrite {
        return Err(precondition_failed(format!(
            "target already exists and overwrite is disabled: {}",
            final_path.display()
        )));
    }

    let effective_chunk_size = clamp_chunk_size(requested_chunk_size, policy);

    // Deterministic .part name so an interrupted upload can resume: it is
    // keyed by the whole-file sha, not the (random) session id, so a fresh
    // begin after a crash re-attaches the existing bytes.
    let part_name = format!(
        ".bifrost-upload.{}.part",
        &total_sha256[..total_sha256.len().min(32)]
    );
    let part_path = parent.join(part_name);

    let received_offset = match fs::metadata(&part_path).await {
        Ok(md) => md.len(),
        Err(_) => 0,
    };
    // A stale .part larger than the declared size is unusable; start over.
    let received_offset = if received_offset > total_size {
        let _ = fs::remove_file(&part_path).await;
        0
    } else {
        received_offset
    };

    let prior_mode = capture_mode(&final_path).await;
    let id = random_id();
    let session = TransferSession {
        kind: TransferKind::Upload,
        final_path,
        part_path: Some(part_path),
        total_size,
        total_sha256,
        chunk_size: effective_chunk_size,
        prior_mode,
        last_activity: Instant::now(),
    };
    {
        let mut map = sessions().lock().unwrap();
        evict_expired(&mut map);
        map.insert(id.clone(), session);
    }

    Ok(json!({
        "upload_id": id,
        "effective_chunk_size": effective_chunk_size,
        "received_offset": received_offset,
        "total_size": total_size,
    }))
}

/// Entry point for `file.download_begin`.
pub(crate) async fn handle_download_begin(
    decision: &PolicyDecision,
    policy: &FileAccessPolicy,
    requested_chunk_size: Option<u64>,
) -> Result<Value> {
    let path = decision.path.as_path().to_path_buf();
    let md = fs::metadata(&path).await.map_err(|_| {
        not_found(format!(
            "download source does not exist: {}",
            path.display()
        ))
    })?;
    if md.is_dir() {
        return Err(invalid_args(format!(
            "download source is a directory: {}",
            path.display()
        )));
    }
    let total_size = md.len();
    if total_size > policy.max_transfer_bytes {
        return Err(size_too_large(format!(
            "file size {} exceeds max_transfer_bytes {}",
            total_size, policy.max_transfer_bytes
        )));
    }
    let total_sha256 = sha256_file(&path).await?;
    let effective_chunk_size = clamp_chunk_size(requested_chunk_size, policy);

    let id = random_id();
    let session = TransferSession {
        kind: TransferKind::Download,
        final_path: path,
        part_path: None,
        total_size,
        total_sha256: total_sha256.clone(),
        chunk_size: effective_chunk_size,
        prior_mode: None,
        last_activity: Instant::now(),
    };
    {
        let mut map = sessions().lock().unwrap();
        evict_expired(&mut map);
        map.insert(id.clone(), session);
    }

    Ok(json!({
        "download_id": id,
        "total_size": total_size,
        "total_sha256": total_sha256,
        "effective_chunk_size": effective_chunk_size,
    }))
}

/// Clamp a requested chunk size to `[1, transfer_chunk_max_bytes]`, falling
/// back to `DEFAULT_CHUNK_SIZE` when the caller omits it or requests 0.
fn clamp_chunk_size(requested: Option<u64>, policy: &FileAccessPolicy) -> u64 {
    let cap = policy.transfer_chunk_max_bytes.max(1);
    let req = requested.filter(|v| *v > 0).unwrap_or(DEFAULT_CHUNK_SIZE);
    req.min(cap).max(1)
}

/// Dispatch for the session-scoped ops (chunk / commit / abort / status for
/// upload; chunk for download).
pub(crate) async fn handle_transfer_session_op(
    op_name: &str,
    params: TransferOpParams,
) -> Result<Value> {
    match op_name {
        "file.upload_chunk" => upload_chunk(params).await,
        "file.upload_commit" => upload_commit(params).await,
        "file.upload_abort" => upload_abort(params).await,
        "file.upload_status" => upload_status(params).await,
        "file.download_chunk" => download_chunk(params).await,
        other => Err(invalid_args(format!("unknown transfer op: {}", other))),
    }
}

fn take_id(params: &TransferOpParams) -> Result<String> {
    params
        .transfer_id
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid_args("'transfer_id' is required"))
}

/// Look up a session, refresh its activity clock, and clone the fields needed
/// without holding the lock across `.await`.
fn snapshot_session(
    id: &str,
) -> Result<(TransferKind, PathBuf, Option<PathBuf>, u64, String, u64)> {
    let mut map = sessions().lock().unwrap();
    evict_expired(&mut map);
    let s = map
        .get_mut(id)
        .ok_or_else(|| invalid_args(format!("unknown transfer_id: {}", id)))?;
    s.last_activity = Instant::now();
    Ok((
        s.kind,
        s.final_path.clone(),
        s.part_path.clone(),
        s.total_size,
        s.total_sha256.clone(),
        s.chunk_size,
    ))
}

async fn upload_chunk(params: TransferOpParams) -> Result<Value> {
    let id = take_id(&params)?;
    let (kind, _final_path, part_path, total_size, _sha, _chunk_size) = snapshot_session(&id)?;
    if kind != TransferKind::Upload {
        return Err(invalid_args("transfer_id is not an upload session"));
    }
    let part_path = part_path.ok_or_else(|| invalid_args("upload session missing part path"))?;
    let offset = params
        .offset
        .ok_or_else(|| invalid_args("'offset' is required for file.upload_chunk"))?;
    let chunk_b64 = params
        .chunk_b64
        .ok_or_else(|| invalid_args("'chunk_b64' is required for file.upload_chunk"))?;
    let chunk = b64_decode(&chunk_b64)?;

    // Per-chunk integrity: verify before touching disk.
    if let Some(expected) = params.chunk_sha256.as_deref() {
        let actual = sha256_hex(&chunk);
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(sha_mismatch(format!(
                "chunk sha mismatch at offset {}: expected {}, got {}",
                offset, expected, actual
            )));
        }
    }

    // Strict, in-order append: offset must equal the current part size.
    let part_size = match fs::metadata(&part_path).await {
        Ok(md) => md.len(),
        Err(_) => 0,
    };
    if offset != part_size {
        return Err(precondition_failed(format!(
            "offset {} does not match current part size {} (out-of-order or duplicate chunk)",
            offset, part_size
        )));
    }
    if part_size + (chunk.len() as u64) > total_size {
        return Err(precondition_failed(format!(
            "chunk would overflow declared total_size {} (part {} + chunk {})",
            total_size,
            part_size,
            chunk.len()
        )));
    }

    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&part_path)
        .await
        .map_err(|e| io_err(&format!("open part {}", part_path.display()), e))?;
    f.write_all(&chunk)
        .await
        .map_err(|e| io_err(&format!("write part {}", part_path.display()), e))?;
    f.flush()
        .await
        .map_err(|e| io_err(&format!("flush part {}", part_path.display()), e))?;

    let next_offset = offset + chunk.len() as u64;
    Ok(json!({ "next_offset": next_offset }))
}

async fn upload_commit(params: TransferOpParams) -> Result<Value> {
    let id = take_id(&params)?;
    let (kind, final_path, part_path, total_size, declared_sha, _chunk_size) =
        snapshot_session(&id)?;
    if kind != TransferKind::Upload {
        return Err(invalid_args("transfer_id is not an upload session"));
    }
    let part_path = part_path.ok_or_else(|| invalid_args("upload session missing part path"))?;

    let part_size = match fs::metadata(&part_path).await {
        Ok(md) => md.len(),
        Err(_) => 0,
    };
    if part_size != total_size {
        return Err(precondition_failed(format!(
            "part size {} does not match declared total_size {}",
            part_size, total_size
        )));
    }

    // Whole-file integrity. Prefer the caller-supplied expected sha, else the
    // sha captured at begin. On mismatch keep the .part for a retry.
    let expected_sha = params.total_sha256.as_deref().unwrap_or(&declared_sha);
    let actual_sha = sha256_file(&part_path).await?;
    if !actual_sha.eq_ignore_ascii_case(expected_sha) {
        return Err(sha_mismatch(format!(
            "whole-file sha mismatch: expected {}, got {} (part retained for retry)",
            expected_sha, actual_sha
        )));
    }

    // Restore/set mode on the part before rename so the final inode already
    // carries the right permission bits.
    let prior_mode = {
        let map = sessions().lock().unwrap();
        map.get(&id).and_then(|s| s.prior_mode)
    };
    apply_mode(&part_path, prior_mode).await;

    fs::rename(&part_path, &final_path)
        .await
        .map_err(|e| io_err(&format!("rename part -> {}", final_path.display()), e))?;

    // Session done.
    {
        let mut map = sessions().lock().unwrap();
        map.remove(&id);
    }

    Ok(json!({
        "path": final_path.to_string_lossy(),
        "bytes_written": total_size,
        "sha256": actual_sha,
    }))
}

async fn upload_abort(params: TransferOpParams) -> Result<Value> {
    let id = take_id(&params)?;
    let part_path = {
        let mut map = sessions().lock().unwrap();
        evict_expired(&mut map);
        map.remove(&id).and_then(|s| s.part_path)
    };
    if let Some(p) = part_path {
        let _ = fs::remove_file(&p).await;
    }
    Ok(json!({ "aborted": true }))
}

async fn upload_status(params: TransferOpParams) -> Result<Value> {
    let id = take_id(&params)?;
    let (kind, _final_path, part_path, total_size, _sha, chunk_size) = snapshot_session(&id)?;
    if kind != TransferKind::Upload {
        return Err(invalid_args("transfer_id is not an upload session"));
    }
    let part_path = part_path.ok_or_else(|| invalid_args("upload session missing part path"))?;
    let received_offset = match fs::metadata(&part_path).await {
        Ok(md) => md.len(),
        Err(_) => 0,
    };
    Ok(json!({
        "received_offset": received_offset,
        "total_size": total_size,
        "chunk_size": chunk_size,
    }))
}

async fn download_chunk(params: TransferOpParams) -> Result<Value> {
    let id = take_id(&params)?;
    let (kind, path, _part, total_size, _sha, chunk_size) = snapshot_session(&id)?;
    if kind != TransferKind::Download {
        return Err(invalid_args("transfer_id is not a download session"));
    }
    let offset = params
        .offset
        .ok_or_else(|| invalid_args("'offset' is required for file.download_chunk"))?;
    if offset > total_size {
        return Err(precondition_failed(format!(
            "offset {} exceeds total_size {}",
            offset, total_size
        )));
    }
    let want = params
        .length
        .filter(|v| *v > 0)
        .unwrap_or(chunk_size)
        .min(chunk_size);
    let remaining = total_size - offset;
    let to_read = want.min(remaining) as usize;

    let mut f = fs::File::open(&path)
        .await
        .map_err(|e| io_err(&format!("open {}", path.display()), e))?;
    f.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| io_err(&format!("seek {}", path.display()), e))?;
    let mut buf = vec![0u8; to_read];
    f.read_exact(&mut buf)
        .await
        .map_err(|e| io_err(&format!("read {}", path.display()), e))?;

    let chunk_sha256 = sha256_hex(&buf);
    let next_offset = offset + to_read as u64;
    let eof = next_offset >= total_size;
    if eof {
        let mut map = sessions().lock().unwrap();
        map.remove(&id);
    }
    Ok(json!({
        "chunk_b64": b64_encode(&buf),
        "chunk_sha256": chunk_sha256,
        "next_offset": next_offset,
        "eof": eof,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rw_policy(root: &Path) -> FileAccessPolicy {
        FileAccessPolicy::new_read_write("t", vec![root.to_path_buf()])
    }

    #[test]
    fn clamp_chunk_size_defaults_and_caps() {
        let tmp = std::env::temp_dir();
        let mut p = rw_policy(&tmp);
        p.transfer_chunk_max_bytes = 8 * 1024 * 1024;
        // omitted -> default
        assert_eq!(clamp_chunk_size(None, &p), DEFAULT_CHUNK_SIZE);
        // zero -> default
        assert_eq!(clamp_chunk_size(Some(0), &p), DEFAULT_CHUNK_SIZE);
        // under cap -> passthrough
        assert_eq!(clamp_chunk_size(Some(1024), &p), 1024);
        // over cap -> clamped
        assert_eq!(
            clamp_chunk_size(Some(100 * 1024 * 1024), &p),
            8 * 1024 * 1024
        );
    }

    #[test]
    fn random_id_is_hex_and_unique() {
        let a = random_id();
        let b = random_id();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        // sha256("") = e3b0c442...
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[tokio::test]
    async fn upload_roundtrip_commit_verifies_sha() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.bin");
        let policy = rw_policy(dir.path());
        let decision = policy
            .check(
                &target,
                dir.path(),
                bifrost_core::file_access::FileOp::Upload,
            )
            .unwrap();
        let data = b"hello chunked world".repeat(1000);
        let sha = sha256_hex(&data);

        let begin = handle_upload_begin(
            &decision,
            &policy,
            Some(data.len() as u64),
            Some(&sha),
            Some(8),
            Some(true),
            false,
        )
        .await
        .unwrap();
        let id = begin["upload_id"].as_str().unwrap().to_string();
        let chunk = begin["effective_chunk_size"].as_u64().unwrap() as usize;
        assert_eq!(begin["received_offset"].as_u64().unwrap(), 0);

        let mut offset = 0u64;
        for piece in data.chunks(chunk) {
            let res = upload_chunk(TransferOpParams {
                transfer_id: Some(id.clone()),
                offset: Some(offset),
                length: None,
                chunk_b64: Some(b64_encode(piece)),
                chunk_sha256: Some(sha256_hex(piece)),
                total_sha256: None,
            })
            .await
            .unwrap();
            offset = res["next_offset"].as_u64().unwrap();
        }
        assert_eq!(offset, data.len() as u64);

        let commit = upload_commit(TransferOpParams {
            transfer_id: Some(id.clone()),
            offset: None,
            length: None,
            chunk_b64: None,
            chunk_sha256: None,
            total_sha256: Some(sha.clone()),
        })
        .await
        .unwrap();
        assert_eq!(commit["sha256"].as_str().unwrap(), sha);
        let on_disk = fs::read(&target).await.unwrap();
        assert_eq!(on_disk, data);
    }

    #[tokio::test]
    async fn upload_chunk_rejects_bad_sha_and_out_of_order() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.bin");
        let policy = rw_policy(dir.path());
        let decision = policy
            .check(
                &target,
                dir.path(),
                bifrost_core::file_access::FileOp::Upload,
            )
            .unwrap();
        let data = vec![7u8; 4096];
        let sha = sha256_hex(&data);
        let begin = handle_upload_begin(
            &decision,
            &policy,
            Some(data.len() as u64),
            Some(&sha),
            Some(1024),
            Some(true),
            false,
        )
        .await
        .unwrap();
        let id = begin["upload_id"].as_str().unwrap().to_string();

        // Bad per-chunk sha.
        let bad = upload_chunk(TransferOpParams {
            transfer_id: Some(id.clone()),
            offset: Some(0),
            length: None,
            chunk_b64: Some(b64_encode(&data[..1024])),
            chunk_sha256: Some("deadbeef".into()),
            total_sha256: None,
        })
        .await;
        assert!(format!("{:?}", bad.unwrap_err()).contains("file.sha_mismatch"));

        // Out-of-order offset.
        let ooo = upload_chunk(TransferOpParams {
            transfer_id: Some(id.clone()),
            offset: Some(9999),
            length: None,
            chunk_b64: Some(b64_encode(&data[..1024])),
            chunk_sha256: Some(sha256_hex(&data[..1024])),
            total_sha256: None,
        })
        .await;
        assert!(format!("{:?}", ooo.unwrap_err()).contains("file.precondition_failed"));
    }

    #[tokio::test]
    async fn download_roundtrip_chunks_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let data = b"download me please".repeat(500);
        fs::write(&src, &data).await.unwrap();
        let policy = rw_policy(dir.path());
        let decision = policy
            .check(
                &src,
                dir.path(),
                bifrost_core::file_access::FileOp::Download,
            )
            .unwrap();
        let begin = handle_download_begin(&decision, &policy, Some(4096))
            .await
            .unwrap();
        // The caller-requested chunk size must be honoured (clamped) rather
        // than silently replaced by the server default — otherwise a large
        // default frame overruns the Relay's per-frame body limit.
        assert_eq!(begin["effective_chunk_size"].as_u64().unwrap(), 4096);
        let id = begin["download_id"].as_str().unwrap().to_string();
        let total = begin["total_size"].as_u64().unwrap();
        let whole_sha = begin["total_sha256"].as_str().unwrap().to_string();

        let mut got = Vec::new();
        let mut offset = 0u64;
        while offset < total {
            let res = download_chunk(TransferOpParams {
                transfer_id: Some(id.clone()),
                offset: Some(offset),
                length: Some(4096),
                chunk_b64: None,
                chunk_sha256: None,
                total_sha256: None,
            })
            .await
            .unwrap();
            let piece = b64_decode(res["chunk_b64"].as_str().unwrap()).unwrap();
            assert_eq!(res["chunk_sha256"].as_str().unwrap(), sha256_hex(&piece));
            got.extend_from_slice(&piece);
            offset = res["next_offset"].as_u64().unwrap();
        }
        assert_eq!(got, data);
        assert_eq!(sha256_hex(&got), whole_sha);
    }

    #[tokio::test]
    async fn upload_begin_rejects_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("big.bin");
        let mut policy = rw_policy(dir.path());
        policy.max_transfer_bytes = 10;
        let decision = policy
            .check(
                &target,
                dir.path(),
                bifrost_core::file_access::FileOp::Upload,
            )
            .unwrap();
        let err = handle_upload_begin(
            &decision,
            &policy,
            Some(1000),
            Some("ab"),
            None,
            Some(true),
            false,
        )
        .await
        .unwrap_err();
        assert!(format!("{:?}", err).contains("file.size_too_large"));
    }
}
