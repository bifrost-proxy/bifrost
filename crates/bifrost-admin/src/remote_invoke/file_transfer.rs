//! Chunked large-file transfer for the Remote Invoke subsystem (Phase 4).
//!
//! The single-shot `file.read` / `file.write` handlers cap out at the Relay
//! per-call limit minus base64 (4/3) and POP-envelope overhead. This module
//! implements resumable, chunked upload (caller -> remote) and download
//! (remote -> caller) so arbitrarily large files can be moved in independent
//! sub-limit remote-invoke calls, with per-chunk and whole-file sha256
//! integrity.
//!
//! ## Throughput / packet-size optimizations (Phase 4.1)
//!
//! The Relay is a content-agnostic dumb pipe, so the application layer owns how
//! payloads are encoded, compressed and paced:
//!
//! * **Adaptive per-chunk zstd** — each chunk is compressed only when zstd
//!   actually shrinks it (already-compressed data self-falls-back to `none`),
//!   tagged with a `chunk_encoding` so the receiver decodes deterministically.
//!   The `chunk_sha256` is always computed over the *raw* (decoded) bytes, so
//!   integrity is independent of the wire encoding.
//! * **Pipelining with in-order landing** — callers keep a bounded window of
//!   chunks in flight to hide relay RTT. Chunks may arrive out of order, but a
//!   bounded reorder buffer lands them to the `.part` file strictly in offset
//!   order, so the on-disk part stays a contiguous prefix and crash-`--resume`
//!   remains a simple "continue from part size".
//! * **Skip-if-identical** — `upload_begin` short-circuits with
//!   `already_complete` when the target already holds a byte-identical file.
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

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use bifrost_core::file_access::{FileAccessPolicy, PolicyDecision};
use bifrost_core::{BifrostError, Result};
use ring::digest::{Context, SHA256};
use serde_json::{json, Value};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Mutex as AsyncMutex;

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
/// under the 2 MiB frame ceiling with headroom for the envelope. Adaptive zstd
/// only ever shrinks the wire payload, so it never pushes a chunk over budget.
const DEFAULT_CHUNK_SIZE: u64 = 512 * 1024;

/// Upper bound on the server-side reorder buffer (out-of-order chunks held in
/// memory per upload session while pipelining). Bounds worst-case memory to
/// `MAX_PENDING_CHUNKS * effective_chunk_size`; a caller whose window exceeds
/// this is told to slow down via `[file.precondition_failed]`.
const MAX_PENDING_CHUNKS: usize = 32;

/// zstd compression level for adaptive chunk compression. Level 3 is the zstd
/// default: a good ratio/speed balance that keeps CPU well under the relay RTT
/// so compression is effectively free on the wire-bound path.
const ZSTD_LEVEL: i32 = 3;

/// Direction of a transfer session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransferKind {
    Upload,
    Download,
}

/// Serialized write path for one upload session. Guarded by an async mutex so
/// concurrently-arriving (pipelined) chunks land to the `.part` file strictly
/// in offset order: out-of-order chunks wait in `pending` until the contiguous
/// prefix reaches them.
struct UploadWriteState {
    part_path: PathBuf,
    /// Bytes already durably appended (== on-disk `.part` length).
    part_size: u64,
    total_size: u64,
    /// Out-of-order chunks keyed by their (raw) start offset.
    pending: BTreeMap<u64, Vec<u8>>,
}

/// In-memory state for one active transfer. Uploads write to a sibling
/// `.part` file for atomic commit + resume; downloads snapshot the source
/// size + whole-file sha at begin for a consistent view.
struct TransferSession {
    kind: TransferKind,
    /// Canonical target (upload) or source (download) path.
    final_path: PathBuf,
    /// `<dir>/.bifrost-upload.<sha-prefix>.part` (upload only).
    part_path: Option<PathBuf>,
    total_size: u64,
    total_sha256: String,
    /// Effective (clamped) chunk size advertised to the caller.
    chunk_size: u64,
    /// Unix mode to restore on the committed file (upload only).
    prior_mode: Option<u32>,
    /// Whether this session may send zstd-compressed chunks on the wire
    /// (upload: server can decode; download: server will compress).
    zstd: bool,
    /// Serialized upload write path (upload only).
    write: Option<Arc<AsyncMutex<UploadWriteState>>>,
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

/// Adaptively compress an outgoing chunk. Returns `(payload, encoding)` where
/// `encoding` is `"zstd"` only when compression actually shrank the data;
/// otherwise the raw bytes are returned with `"none"`. This prevents the
/// classic pitfall of inflating already-compressed content (jpg/mp4/tar.gz).
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
            .map_err(|e| invalid_args(format!("zstd decompress failed: {}", e))),
        other => Err(invalid_args(format!("unknown chunk_encoding: {}", other))),
    }
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

/// Panic-safe `.part` file name derived from a (possibly caller-supplied)
/// whole-file sha. Takes at most the first 32 *characters* (never a byte slice
/// that could split a multi-byte char and panic) so a hostile non-hex sha can
/// never crash the request path.
fn part_file_name(total_sha256: &str) -> String {
    let prefix: String = total_sha256.chars().take(32).collect();
    format!(".bifrost-upload.{}.part", prefix)
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

/// Whether the caller advertised zstd support in its `accept_encodings` list.
fn accepts_zstd(accept_encodings: Option<&[String]>) -> bool {
    accept_encodings
        .map(|list| list.iter().any(|e| e.eq_ignore_ascii_case("zstd")))
        .unwrap_or(false)
}

/// Params carried by the session-scoped transfer ops (everything except
/// `begin`, which resolves a fresh policy decision).
pub(crate) struct TransferOpParams {
    pub transfer_id: Option<String>,
    pub offset: Option<u64>,
    pub length: Option<u64>,
    pub chunk_b64: Option<String>,
    pub chunk_sha256: Option<String>,
    pub chunk_encoding: Option<String>,
    pub total_sha256: Option<String>,
}

/// Entry point for `file.upload_begin`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_upload_begin(
    decision: &PolicyDecision,
    policy: &FileAccessPolicy,
    total_size: Option<u64>,
    total_sha256: Option<&str>,
    requested_chunk_size: Option<u64>,
    allow_overwrite: Option<bool>,
    create_parents: bool,
    accept_encodings: Option<&[String]>,
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
    let effective_chunk_size = clamp_chunk_size(requested_chunk_size, policy);
    // Server can always decode zstd; advertise it so a caller may compress.
    let zstd = accepts_zstd(accept_encodings);

    // Skip-if-identical: if the target already holds byte-identical content,
    // there is nothing to transfer. This makes idempotent re-pushes (e.g. the
    // same build artifact) a single round-trip with zero chunks.
    if fs::try_exists(&final_path).await.unwrap_or(false) {
        if let Ok(existing) = sha256_file(&final_path).await {
            if existing.eq_ignore_ascii_case(&total_sha256) {
                return Ok(json!({
                    "upload_id": random_id(),
                    "effective_chunk_size": effective_chunk_size,
                    "received_offset": total_size,
                    "total_size": total_size,
                    "chunk_encoding": if zstd { "zstd" } else { "none" },
                    "already_complete": true,
                    "path": final_path.to_string_lossy(),
                    "sha256": total_sha256,
                }));
            }
        }
    }

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

    // Deterministic .part name so an interrupted upload can resume: it is
    // keyed by the whole-file sha, not the (random) session id, so a fresh
    // begin after a crash re-attaches the existing bytes.
    let part_name = part_file_name(&total_sha256);
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
    let write = Arc::new(AsyncMutex::new(UploadWriteState {
        part_path: part_path.clone(),
        part_size: received_offset,
        total_size,
        pending: BTreeMap::new(),
    }));
    let session = TransferSession {
        kind: TransferKind::Upload,
        final_path,
        part_path: Some(part_path),
        total_size,
        total_sha256,
        chunk_size: effective_chunk_size,
        prior_mode,
        zstd,
        write: Some(write),
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
        "chunk_encoding": if zstd { "zstd" } else { "none" },
        "already_complete": false,
    }))
}

/// Single round-trip fast path for a small file (`file.upload_small`).
///
/// When the whole file fits inside one relay frame budget, the three-call
/// begin->chunk->commit dance is pure overhead. This handler does the entire
/// upload in one policy-checked call: decode the (optionally zstd) payload,
/// verify the whole-file sha over the raw bytes, then atomically write via a
/// sibling `.part` + rename so a partial write is never observable at the
/// final path. It reuses the exact policy decision, overwrite/create_parents
/// gating, skip-if-identical short-circuit and size limit of `upload_begin`,
/// so no new authorization surface is introduced.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_upload_small(
    decision: &PolicyDecision,
    policy: &FileAccessPolicy,
    total_size: Option<u64>,
    total_sha256: Option<&str>,
    chunk_b64: Option<&str>,
    chunk_encoding: Option<&str>,
    allow_overwrite: Option<bool>,
    create_parents: bool,
) -> Result<Value> {
    let total_size =
        total_size.ok_or_else(|| invalid_args("'total_size' is required for file.upload_small"))?;
    let declared_sha = total_sha256
        .ok_or_else(|| invalid_args("'total_sha256' is required for file.upload_small"))?
        .to_string();
    let chunk_b64 =
        chunk_b64.ok_or_else(|| invalid_args("'chunk_b64' is required for file.upload_small"))?;

    if total_size > policy.max_transfer_bytes {
        return Err(size_too_large(format!(
            "total_size {} exceeds max_transfer_bytes {}",
            total_size, policy.max_transfer_bytes
        )));
    }
    // Guard the fast path to genuinely small files: the whole file is carried
    // in a single frame, so it must fit the clamp ceiling. Larger files must
    // use the chunked begin/chunk/commit protocol.
    let budget = clamp_chunk_size(None, policy).max(policy.transfer_chunk_max_bytes);
    if total_size > budget {
        return Err(precondition_failed(format!(
            "total_size {} exceeds the single-frame fast-path budget {}; use chunked upload",
            total_size, budget
        )));
    }

    // Decode the payload; the raw size may never exceed the declared total.
    let payload = b64_decode(chunk_b64)?;
    let raw = decode_chunk(&payload, chunk_encoding, total_size as usize)?;
    if raw.len() as u64 != total_size {
        return Err(precondition_failed(format!(
            "decoded payload is {} bytes but total_size is {}",
            raw.len(),
            total_size
        )));
    }
    // Whole-file integrity over the RAW bytes (encoding-independent).
    let actual_sha = sha256_hex(&raw);
    if !actual_sha.eq_ignore_ascii_case(&declared_sha) {
        return Err(sha_mismatch(format!(
            "whole-file sha mismatch: expected {}, got {}",
            declared_sha, actual_sha
        )));
    }

    let final_path = decision.path.as_path().to_path_buf();
    let overwrite = allow_overwrite.unwrap_or(decision.allow_overwrite);

    // Skip-if-identical: nothing to write.
    if fs::try_exists(&final_path).await.unwrap_or(false) {
        if let Ok(existing) = sha256_file(&final_path).await {
            if existing.eq_ignore_ascii_case(&declared_sha) {
                return Ok(json!({
                    "path": final_path.to_string_lossy(),
                    "bytes_written": total_size,
                    "sha256": actual_sha,
                    "already_complete": true,
                }));
            }
        }
    }

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
    if fs::try_exists(&final_path).await.unwrap_or(false) && !overwrite {
        return Err(precondition_failed(format!(
            "target already exists and overwrite is disabled: {}",
            final_path.display()
        )));
    }

    // Atomic publish: write to a sibling .part then rename. Reuse the sha-keyed
    // part name so a crashed fast-path write leaves no colliding debris.
    let prior_mode = capture_mode(&final_path).await;
    let part_path = parent.join(part_file_name(&declared_sha));
    fs::write(&part_path, &raw)
        .await
        .map_err(|e| io_err(&format!("write part {}", part_path.display()), e))?;
    apply_mode(&part_path, prior_mode).await;
    if let Err(e) = fs::rename(&part_path, &final_path).await {
        // Best-effort cleanup so a failed rename does not leak the .part.
        let _ = fs::remove_file(&part_path).await;
        return Err(io_err(
            &format!("rename part -> {}", final_path.display()),
            e,
        ));
    }

    Ok(json!({
        "path": final_path.to_string_lossy(),
        "bytes_written": total_size,
        "sha256": actual_sha,
        "already_complete": false,
    }))
}

/// Entry point for `file.download_begin`.
pub(crate) async fn handle_download_begin(
    decision: &PolicyDecision,
    policy: &FileAccessPolicy,
    requested_chunk_size: Option<u64>,
    accept_encodings: Option<&[String]>,
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
    // Only compress download chunks when the caller advertised it can decode.
    let zstd = accepts_zstd(accept_encodings);

    let id = random_id();
    let session = TransferSession {
        kind: TransferKind::Download,
        final_path: path,
        part_path: None,
        total_size,
        total_sha256: total_sha256.clone(),
        chunk_size: effective_chunk_size,
        prior_mode: None,
        zstd,
        write: None,
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
        "content_encoding": if zstd { "zstd" } else { "none" },
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

/// Immutable snapshot of a transfer session's fields (returned by
/// [`snapshot_session`]). Excludes the upload write handle, which is cloned
/// separately by [`upload_write_handle`].
struct SessionSnapshot {
    kind: TransferKind,
    final_path: PathBuf,
    part_path: Option<PathBuf>,
    total_size: u64,
    total_sha256: String,
    chunk_size: u64,
    zstd: bool,
}

/// Read-only snapshot of a session's immutable fields, refreshing its activity
/// clock. Does not clone the (upload) write handle.
fn snapshot_session(id: &str) -> Result<SessionSnapshot> {
    let mut map = sessions().lock().unwrap();
    evict_expired(&mut map);
    let s = map
        .get_mut(id)
        .ok_or_else(|| invalid_args(format!("unknown transfer_id: {}", id)))?;
    s.last_activity = Instant::now();
    Ok(SessionSnapshot {
        kind: s.kind,
        final_path: s.final_path.clone(),
        part_path: s.part_path.clone(),
        total_size: s.total_size,
        total_sha256: s.total_sha256.clone(),
        chunk_size: s.chunk_size,
        zstd: s.zstd,
    })
}

/// Clone the upload write handle out of the session map (releasing the global
/// lock before any `.await`), refreshing the activity clock.
fn upload_write_handle(id: &str) -> Result<(Arc<AsyncMutex<UploadWriteState>>, u64)> {
    let mut map = sessions().lock().unwrap();
    evict_expired(&mut map);
    let s = map
        .get_mut(id)
        .ok_or_else(|| invalid_args(format!("unknown transfer_id: {}", id)))?;
    if s.kind != TransferKind::Upload {
        return Err(invalid_args("transfer_id is not an upload session"));
    }
    s.last_activity = Instant::now();
    let handle = s
        .write
        .clone()
        .ok_or_else(|| invalid_args("upload session missing write state"))?;
    Ok((handle, s.chunk_size))
}

async fn upload_chunk(params: TransferOpParams) -> Result<Value> {
    let id = take_id(&params)?;
    let (write, chunk_size) = upload_write_handle(&id)?;

    let offset = params
        .offset
        .ok_or_else(|| invalid_args("'offset' is required for file.upload_chunk"))?;
    let chunk_b64 = params
        .chunk_b64
        .ok_or_else(|| invalid_args("'chunk_b64' is required for file.upload_chunk"))?;
    let payload = b64_decode(&chunk_b64)?;
    // Decode according to the per-chunk encoding tag; `chunk_size` bounds the
    // decompressed size so a hostile chunk cannot blow up memory.
    let chunk = decode_chunk(
        &payload,
        params.chunk_encoding.as_deref(),
        chunk_size as usize,
    )?;

    // Per-chunk integrity over the RAW bytes (encoding-independent), before
    // touching disk.
    if let Some(expected) = params.chunk_sha256.as_deref() {
        let actual = sha256_hex(&chunk);
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(sha_mismatch(format!(
                "chunk sha mismatch at offset {}: expected {}, got {}",
                offset, expected, actual
            )));
        }
    }

    // Serialize the write path: land chunks to the contiguous prefix in order,
    // buffering any that arrive ahead of the current part size.
    let mut st = write.lock().await;

    if offset + (chunk.len() as u64) > st.total_size {
        return Err(precondition_failed(format!(
            "chunk would overflow declared total_size {} (offset {} + chunk {})",
            st.total_size,
            offset,
            chunk.len()
        )));
    }

    // Already-landed bytes (duplicate / resend): idempotent ack.
    if offset < st.part_size {
        let received = st.part_size;
        return Ok(json!({ "next_offset": received, "received_offset": received }));
    }

    // Ahead of the write frontier: buffer it (bounded), do not advance.
    if offset > st.part_size {
        if !st.pending.contains_key(&offset) && st.pending.len() >= MAX_PENDING_CHUNKS {
            return Err(precondition_failed(format!(
                "reorder buffer full ({} chunks); reduce the transfer window",
                MAX_PENDING_CHUNKS
            )));
        }
        st.pending.insert(offset, chunk);
        let received = st.part_size;
        return Ok(json!({ "next_offset": received, "received_offset": received }));
    }

    // offset == part_size: land this chunk, then drain any buffered chunks that
    // have become contiguous.
    st.pending.insert(offset, chunk);
    let part_path = st.part_path.clone();
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&part_path)
        .await
        .map_err(|e| io_err(&format!("open part {}", part_path.display()), e))?;
    loop {
        let next = st.part_size;
        let Some(bytes) = st.pending.remove(&next) else {
            break;
        };
        f.write_all(&bytes)
            .await
            .map_err(|e| io_err(&format!("write part {}", part_path.display()), e))?;
        st.part_size += bytes.len() as u64;
    }
    f.flush()
        .await
        .map_err(|e| io_err(&format!("flush part {}", part_path.display()), e))?;

    let received = st.part_size;
    Ok(json!({ "next_offset": received, "received_offset": received }))
}

async fn upload_commit(params: TransferOpParams) -> Result<Value> {
    let id = take_id(&params)?;
    let snap = snapshot_session(&id)?;
    if snap.kind != TransferKind::Upload {
        return Err(invalid_args("transfer_id is not an upload session"));
    }
    let part_path = snap
        .part_path
        .ok_or_else(|| invalid_args("upload session missing part path"))?;
    let final_path = snap.final_path;
    let total_size = snap.total_size;
    let declared_sha = snap.total_sha256;

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
    let snap = snapshot_session(&id)?;
    if snap.kind != TransferKind::Upload {
        return Err(invalid_args("transfer_id is not an upload session"));
    }
    let part_path = snap
        .part_path
        .ok_or_else(|| invalid_args("upload session missing part path"))?;
    let received_offset = match fs::metadata(&part_path).await {
        Ok(md) => md.len(),
        Err(_) => 0,
    };
    Ok(json!({
        "received_offset": received_offset,
        "total_size": snap.total_size,
        "chunk_size": snap.chunk_size,
    }))
}

async fn download_chunk(params: TransferOpParams) -> Result<Value> {
    let id = take_id(&params)?;
    let snap = snapshot_session(&id)?;
    if snap.kind != TransferKind::Download {
        return Err(invalid_args("transfer_id is not a download session"));
    }
    let path = snap.final_path;
    let total_size = snap.total_size;
    let chunk_size = snap.chunk_size;
    let zstd = snap.zstd;
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

    // sha over the RAW bytes; adaptively compress the wire payload.
    let chunk_sha256 = sha256_hex(&buf);
    let (payload, encoding) = encode_chunk(&buf, zstd);
    let next_offset = offset + to_read as u64;
    let eof = next_offset >= total_size;
    // NOTE: do NOT remove the session on eof — with a pipelined caller the
    // final chunk can complete before earlier ones, and evicting here would
    // fail their in-flight requests. Idle sessions are reaped by the TTL.
    Ok(json!({
        "chunk_b64": b64_encode(&payload),
        "chunk_sha256": chunk_sha256,
        "chunk_encoding": encoding,
        "next_offset": next_offset,
        "eof": eof,
    }))
}

#[cfg(test)]
#[path = "file_transfer_tests.rs"]
mod tests;
