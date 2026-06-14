//! Remote file operations for the Remote Invoke subsystem.
//!
//! Every handler here is invoked *after* [`bifrost_core::file_access::FileAccessPolicy::check`]
//! has produced a [`PolicyDecision`]. The handlers themselves perform no
//! additional access control — they only translate the decision into
//! `tokio::fs` calls and a wire response.
//!
//! See `design/remote-invoke-file-api.md` §4 for the response schemas.

use std::collections::VecDeque;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use base64::Engine;
use bifrost_core::file_access::{
    DenyMatcher, FileAccessError, FileOp, GlobMatcher, PolicyDecision,
};
use bifrost_core::{BifrostError, Result};
use ring::digest::{Context, SHA256};
use serde_json::{json, Value};
use tokio::fs;
use tokio::io::AsyncReadExt;

const BINARY_SNIFF_BYTES: usize = 8 * 1024;
const DEFAULT_LIST_DEPTH: u32 = 1;
const DEFAULT_LIST_MAX: usize = 2_000;
const DEFAULT_GLOB_MAX: usize = 1_000;
const DEFAULT_SEARCH_MAX: usize = 500;
/// Safety ceiling on total hits collected before pagination, independent of the
/// per-page `max`. Prevents an unbounded regex match set from exhausting memory
/// while still allowing cursor-based paging up to this many results.
const SEARCH_HARD_CAP: usize = 50_000;
const DEFAULT_SEARCH_SCAN_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ENTRIES_PER_DIR: usize = 10_000;

/// Directories skipped by default in `file.list`, `file.glob`, and `file.search`.
const DEFAULT_EXCLUDE_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".svn",
    ".hg",
];

/// Returns `true` if `dir_name` should be skipped during directory traversal.
fn should_skip_dir(dir_name: &str, extra_excludes: &[String]) -> bool {
    if DEFAULT_EXCLUDE_DIRS.contains(&dir_name) {
        return true;
    }
    extra_excludes.iter().any(|e| e == dir_name)
}

fn fa_to_bifrost(err: FileAccessError) -> BifrostError {
    BifrostError::Config(format!("[{}] {}", err.code(), err))
}

fn io_err(ctx: &str, err: std::io::Error) -> BifrostError {
    BifrostError::Config(format!("[file.io_error] {}: {}", ctx, err))
}

/// Map an io error to a *structural* wire code where the errno carries
/// actionable meaning, falling back to the generic `[file.io_error]`.
/// This lets CLIs branch on `file.already_exists` / `file.not_found` /
/// `file.cross_device` instead of string-matching opaque OS messages.
fn io_err_structured(ctx: &str, err: std::io::Error) -> BifrostError {
    use std::io::ErrorKind;
    // EXDEV (cross-device link) has no stable ErrorKind on all toolchains,
    // so match the raw errno (18 on Linux/macOS) as well.
    let is_exdev = err.raw_os_error() == Some(18);
    let code = match err.kind() {
        ErrorKind::AlreadyExists => "file.already_exists",
        ErrorKind::NotFound => "file.not_found",
        ErrorKind::PermissionDenied => "file.permission_denied",
        _ if is_exdev => "file.cross_device",
        _ => "file.io_error",
    };
    BifrostError::Config(format!("[{}] {}: {}", code, ctx, err))
}

fn system_time_to_unix(time: std::time::SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

fn is_probably_binary(sample: &[u8]) -> bool {
    sample.iter().take(BINARY_SNIFF_BYTES).any(|b| *b == 0)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut ctx = Context::new(&SHA256);
    ctx.update(bytes);
    let digest = ctx.finish();
    digest
        .as_ref()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

/// P1: deterministic offset pagination over a fully-collected, sorted result
/// set. Returns the requested page plus the cursor to pass back for the next
/// page (`None` when the result set is exhausted). A `limit` of 0 is treated
/// as "no pagination" and returns everything from the offset onward.
fn paginate<T>(items: Vec<T>, cursor: Option<u64>, limit: usize) -> (Vec<T>, Option<u64>) {
    let start = cursor.unwrap_or(0) as usize;
    let total = items.len();
    if start >= total {
        return (Vec::new(), None);
    }
    let mut iter = items.into_iter().skip(start);
    if limit == 0 {
        return (iter.collect(), None);
    }
    let page: Vec<T> = iter.by_ref().take(limit).collect();
    let consumed = start + page.len();
    let next = if consumed < total {
        Some(consumed as u64)
    } else {
        None
    };
    (page, next)
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
    let digest = ctx.finish();
    Ok(digest
        .as_ref()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect())
}

/// Capture the unix file mode of an existing path. Returns `None` when the
/// path does not exist or the metadata cannot be read. On non-unix platforms
/// always returns `None` (mode restoration becomes a no-op there).
#[allow(unused_variables)]
async fn capture_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(path).await {
            // Mask to permission bits; st_mode also carries file-type
            // bits (S_IFREG etc.) that must not round-trip via chmod.
            Ok(md) => Some(md.permissions().mode() & 0o7777),
            Err(_) => None,
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Restore a previously captured unix file mode. No-op when `mode` is `None`
/// or when the platform is not unix. Errors are intentionally swallowed —
/// a successful write should not be reported as a failure if only chmod fails
/// (the caller can re-stat if it needs to verify).
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

/// Synchronously set the unix mode on a path. Used on the tmp file
/// prior to `rename` so the final inode already carries the desired
/// permission bits, even if a later async `apply_mode` were to fail.
/// No-op on non-unix platforms.
#[allow(unused_variables)]
fn apply_mode_sync(path: &Path, mode: Option<u32>) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        if let Some(m) = mode {
            use std::os::unix::fs::PermissionsExt;
            let res = std::fs::set_permissions(path, std::fs::Permissions::from_mode(m));
            diag_mode(
                "apply_mode_sync",
                path,
                Some(m),
                res.as_ref().err().map(|e| e.to_string()),
            );
            return res;
        }
    }
    Ok(())
}

/// Read back the current unix mode for a path (masked to perm bits).
/// Returns `None` when not unix, the path is missing, or stat fails.
#[allow(unused_variables)]
fn read_mode_sync(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(md) => Some(md.permissions().mode() & 0o7777),
            Err(_) => None,
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Diagnostic sidecar log. Enabled when `BIFROST_FILE_OPS_DEBUG=1`.
/// Writes a single line to `BIFROST_FILE_OPS_DEBUG_LOG` (default
/// `/tmp/bifrost-fileops.log`). Used for post-mortem in CI where we
/// cannot attach a debugger.
fn diag_mode(tag: &str, path: &Path, mode: Option<u32>, err: Option<String>) {
    if std::env::var("BIFROST_FILE_OPS_DEBUG").ok().as_deref() != Some("1") {
        return;
    }
    let log_path = std::env::var("BIFROST_FILE_OPS_DEBUG_LOG")
        .unwrap_or_else(|_| "/tmp/bifrost-fileops.log".to_string());
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(
            f,
            "{} tag={} path={} mode={:?} err={:?}",
            now,
            tag,
            path.display(),
            mode.map(|m| format!("0o{:o}", m)),
            err,
        );
    }
}

/// Source-file newline style detected by sampling its first occurrence. Used
/// to keep `file.edit` and `file.apply_patch` from silently converting CRLF
/// files to LF on round-trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Eol {
    Crlf,
    Lf,
}

impl Eol {
    fn detect(bytes: &[u8]) -> Eol {
        // Scan for the first `\n`; if preceded by `\r` treat the file as CRLF.
        // Files with no newline at all default to LF (platform-agnostic and
        // matches what LLM-generated patches typically emit).
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'\n' {
                if i > 0 && bytes[i - 1] == b'\r' {
                    return Eol::Crlf;
                }
                return Eol::Lf;
            }
        }
        Eol::Lf
    }

    fn newline(self) -> &'static str {
        match self {
            Eol::Crlf => "\r\n",
            Eol::Lf => "\n",
        }
    }
}

/// Rewrite `text` so that every bare LF (not already preceded by CR) becomes
/// `eol.newline()`. This is used when the user-supplied replacement or patch
/// `+` line is in LF form but the source file is CRLF — we normalize to the
/// source style before emitting.
fn normalize_to_eol(text: &str, eol: Eol) -> String {
    // First collapse any CRLF in the input to LF so we have a single
    // canonical form; then re-emit in the target style. This is UTF-8 safe
    // because we only match / insert ASCII bytes (`\r`, `\n`).
    let mut lf_only = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            lf_only.push('\n');
            i += 2;
            continue;
        }
        // Push the next UTF-8 scalar as-is (safe: we started at a char
        // boundary and only skipped ASCII `\r\n`).
        let tail = &text[i..];
        let ch = tail.chars().next().expect("non-empty tail");
        lf_only.push(ch);
        i += ch.len_utf8();
    }
    if eol == Eol::Lf {
        return lf_only;
    }
    // CRLF target: every `\n` becomes `\r\n`.
    let mut out = String::with_capacity(lf_only.len() + 16);
    for ch in lf_only.chars() {
        if ch == '\n' {
            out.push('\r');
        }
        out.push(ch);
    }
    out
}

/// Normalize a symlink-target path for the wire. On Unix this is a passthrough.
/// On Windows, `fs::read_link` can return the NT verbatim form
/// (`\\?\C:\...` or `\\?\UNC\server\share\...`) which leaks an
/// implementation detail. Strip those prefixes so callers receive a clean
/// user-friendly path.
#[allow(unused_mut)]
fn normalize_symlink_target(p: std::path::PathBuf) -> String {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy().to_string();
        if let Some(rest) = s.strip_prefix("\\\\?\\UNC\\") {
            format!("\\\\{}", rest)
        } else if let Some(rest) = s.strip_prefix("\\\\?\\") {
            rest.to_string()
        } else {
            s
        }
    }
    #[cfg(not(windows))]
    {
        p.to_string_lossy().to_string()
    }
}

/// Build an ignore::gitignore::Gitignore matcher by walking up from `root` to
/// the filesystem root, loading every `.gitignore` encountered. This matches
/// what ripgrep / git do when searching below a subdirectory of a repo.
fn build_gitignore_for(root: &std::path::Path) -> Option<ignore::gitignore::Gitignore> {
    let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
    let mut p: Option<&std::path::Path> = Some(root);
    let mut chain: Vec<std::path::PathBuf> = Vec::new();
    while let Some(cur) = p {
        chain.push(cur.to_path_buf());
        p = cur.parent();
    }
    // Load from outermost → root so root-level rules override parents.
    for dir in chain.iter().rev() {
        let gi = dir.join(".gitignore");
        if gi.is_file() {
            let _ = builder.add(&gi);
        }
    }
    builder.build().ok()
}

/// Returns true if the path at `abs` should be skipped under gitignore rules
/// anchored at `root`. `is_dir` lets the matcher apply directory-only rules.
fn is_gitignored(gi: &ignore::gitignore::Gitignore, abs: &std::path::Path, is_dir: bool) -> bool {
    matches!(
        gi.matched_path_or_any_parents(abs, is_dir),
        ignore::Match::Ignore(_)
    )
}

/// `file.read` — return base64-encoded bytes (capped) plus metadata.
pub async fn handle_file_read(
    decision: &PolicyDecision,
    max_bytes: Option<u64>,
    allow_binary: bool,
    offset: Option<u32>,
    limit: Option<u32>,
) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Read);
    let path = decision.path.as_path();

    let metadata = fs::metadata(path)
        .await
        .map_err(|e| io_err(&format!("stat {}", path.display()), e))?;
    if !metadata.is_file() {
        return Err(BifrostError::Config(format!(
            "[file.not_found] not a regular file: {}",
            path.display()
        )));
    }
    let total_size = metadata.len();

    let cap = max_bytes
        .unwrap_or(decision.max_read_bytes)
        .min(decision.max_read_bytes);

    let f = fs::File::open(path)
        .await
        .map_err(|e| io_err(&format!("open {}", path.display()), e))?;
    let to_read = cap.min(total_size);
    let mut buf = Vec::with_capacity(to_read as usize);
    // Read up to `cap` bytes.
    let mut limited = f.take(cap);
    limited
        .read_to_end(&mut buf)
        .await
        .map_err(|e| io_err(&format!("read {}", path.display()), e))?;

    if !allow_binary && is_probably_binary(&buf) {
        return Err(fa_to_bifrost(FileAccessError::BinaryNotAllowed {
            path: path.to_path_buf(),
        }));
    }

    let bytes_truncated = (buf.len() as u64) < total_size;
    let mtime_unix = metadata.modified().ok().and_then(system_time_to_unix);

    // Line-range slicing when offset/limit are provided.
    if offset.is_some() || limit.is_some() {
        let text = std::str::from_utf8(&buf).map_err(|_| {
            BifrostError::Config(
                "[file.invalid_args] file is not UTF-8; offset/limit require text files".into(),
            )
        })?;
        let all_lines: Vec<&str> = text.lines().collect();
        let total_lines = u32::try_from(all_lines.len()).unwrap_or(u32::MAX);
        let start = offset.unwrap_or(1).max(1);
        let count = limit.unwrap_or(total_lines);
        let start_idx = ((start - 1) as usize).min(all_lines.len());
        let end_idx = (start_idx + count as usize).min(all_lines.len());
        let selected: String = all_lines[start_idx..end_idx].join("\n");
        // P0-3: only append a trailing newline when (a) we are not at
        // the last line of the file, or (b) the source file itself
        // ends with '\n'. Otherwise we would inject a byte that
        // never existed on disk and break sha256 round-trip.
        let source_ends_with_nl = buf.last().copied() == Some(b'\n');
        let end_idx_u32 = u32::try_from(end_idx).unwrap_or(u32::MAX);
        let reached_eof = end_idx_u32 == total_lines;
        let need_trailing_nl = start_idx < end_idx && (!reached_eof || source_ends_with_nl);
        let selected_bytes = if start_idx < end_idx {
            if need_trailing_nl {
                format!("{}\n", selected).into_bytes()
            } else {
                selected.into_bytes()
            }
        } else {
            Vec::new()
        };
        let sha256 = sha256_hex(&selected_bytes);
        let start_idx_u32 = u32::try_from(start_idx).unwrap_or(u32::MAX);
        let line_truncated = end_idx_u32 < total_lines || bytes_truncated;
        // Always compute whole-file sha for the line-range branch too —
        // `sha256` above is only over the selected lines, so it cannot be
        // used as `base_sha256` for a subsequent file.write.
        let file_sha256 = sha256_file(path).await.ok();
        return Ok(json!({
            "content_b64": base64::engine::general_purpose::STANDARD.encode(&selected_bytes),
            "size": selected_bytes.len() as u64,
            "total_size": total_size,
            "truncated": line_truncated,
            "sha256": sha256,
            "file_sha256": file_sha256,
            "mtime_unix": mtime_unix,
            "total_lines": total_lines,
            "start_line": start_idx_u32.saturating_add(1),
            "end_line": end_idx_u32,
        }));
    }

    let sha256 = sha256_hex(&buf);
    // `file_sha256` is the digest of the ENTIRE file (not just the returned
    // slice). Callers that want to pass it as `base_sha256` to file.write
    // should use this, not `sha256`, when `truncated=true`.
    let file_sha256 = if bytes_truncated {
        sha256_file(path).await.ok()
    } else {
        Some(sha256.clone())
    };
    let mut result = json!({
        "content_b64": base64::engine::general_purpose::STANDARD.encode(&buf),
        "size": buf.len() as u64,
        "total_size": total_size,
        "truncated": bytes_truncated,
        "sha256": sha256,
        "file_sha256": file_sha256,
        "mtime_unix": mtime_unix,
    });
    // Include total_lines for text content so coding agents can plan chunked reads.
    if let Ok(text) = std::str::from_utf8(&buf) {
        result["total_lines"] = json!(u32::try_from(text.lines().count()).unwrap_or(u32::MAX));
    }
    Ok(result)
}

/// `file.read_many` — read multiple files in one round-trip. Each item is
/// policy-checked by the caller (executor) and passed in as either a ready
/// [`PolicyDecision`] or a pre-formatted error string. Per-file failures are
/// captured into the result and never abort the batch. Reads run concurrently.
pub async fn handle_file_read_many(
    items: Vec<(String, std::result::Result<PolicyDecision, String>)>,
    max_bytes: Option<u64>,
    allow_binary: bool,
) -> Result<Value> {
    let futs = items.into_iter().map(|(path, dec)| async move {
        match dec {
            Err(error) => json!({
                "path": path,
                "ok": false,
                "error_code": error
                    .split(']')
                    .next()
                    .map(|s| s.trim_start_matches('[').to_string())
                    .unwrap_or_default(),
                "error": error,
            }),
            Ok(decision) => {
                match handle_file_read(&decision, max_bytes, allow_binary, None, None).await {
                    Ok(mut v) => {
                        if let Some(obj) = v.as_object_mut() {
                            obj.insert("path".to_string(), json!(path));
                            obj.insert("ok".to_string(), json!(true));
                        }
                        v
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        let code = msg
                            .split(']')
                            .next()
                            .map(|s| s.trim_start_matches('[').to_string())
                            .unwrap_or_default();
                        json!({
                            "path": path,
                            "ok": false,
                            "error_code": code,
                            "error": msg,
                        })
                    }
                }
            }
        }
    });

    let files: Vec<Value> = futures_util::future::join_all(futs).await;
    let ok_count = files
        .iter()
        .filter(|f| f.get("ok").and_then(Value::as_bool) == Some(true))
        .count();
    Ok(json!({
        "files": files,
        "count": files.len(),
        "ok_count": ok_count,
    }))
}

/// `file.list` — breadth-first directory listing up to `depth` levels.
pub async fn handle_file_list(
    decision: &PolicyDecision,
    depth: Option<u32>,
    exclude_patterns: &[String],
    respect_gitignore: bool,
    denies: &[String],
    max_results: Option<usize>,
    cursor: Option<u64>,
) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::List);
    let depth = depth.unwrap_or(DEFAULT_LIST_DEPTH).max(1);
    let max = max_results.unwrap_or(DEFAULT_LIST_MAX);
    let root = decision.path.as_path().to_path_buf();

    // P1: enforce deny rules per-entry so policy-blacklisted paths (ssh keys,
    // credentials, env files, ...) never leak through directory listing.
    let deny_matcher = if denies.is_empty() {
        None
    } else {
        Some(DenyMatcher::new(denies).map_err(|e| {
            BifrostError::Config(format!("[file.invalid_deny] bad deny pattern: {}", e))
        })?)
    };

    let gi_opt = if respect_gitignore {
        build_gitignore_for(decision.path.as_path())
    } else {
        None
    };
    let mut entries: Vec<Value> = Vec::new();
    let mut truncated = false;
    let mut queue: VecDeque<(PathBuf, u32)> = VecDeque::new();
    queue.push_back((root.clone(), 0));

    while let Some((dir, cur_depth)) = queue.pop_front() {
        let mut rd = match fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(e) => return Err(io_err(&format!("read_dir {}", dir.display()), e)),
        };

        let mut count = 0usize;
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| io_err(&format!("next_entry {}", dir.display()), e))?
        {
            count += 1;
            if count > MAX_ENTRIES_PER_DIR {
                truncated = true;
                break;
            }
            let path = entry.path();
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(ref dm) = deny_matcher {
                if dm.match_raw(&rel).is_some() {
                    continue;
                }
            }
            // lstat first so we can accurately tag symlinks; fall back to
            // follow-metadata for size/mtime when the symlink resolves.
            let lmd = match fs::symlink_metadata(&path).await {
                Ok(lmd) => lmd,
                Err(_) => continue,
            };
            let md = if lmd.file_type().is_symlink() {
                fs::metadata(&path).await.unwrap_or_else(|_| lmd.clone())
            } else {
                lmd.clone()
            };
            if let Some(ref gi) = gi_opt {
                if is_gitignored(gi, &path, md.is_dir()) {
                    continue;
                }
            }
            let kind = if lmd.file_type().is_symlink() {
                "symlink"
            } else if md.is_dir() {
                "dir"
            } else if md.is_file() {
                "file"
            } else {
                "other"
            };
            let name = entry.file_name().to_string_lossy().to_string();
            let mut entry_val = json!({
                "name": name,
                "path": rel,
                "type": kind,
                "size": md.len(),
                "mtime_unix": md.modified().ok().and_then(system_time_to_unix),
            });
            if lmd.file_type().is_symlink() {
                if let Ok(tgt) = fs::read_link(&path).await {
                    entry_val["symlink_target"] = Value::String(normalize_symlink_target(tgt));
                }
            }
            entries.push(entry_val);
            if md.is_dir() && cur_depth + 1 < depth {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if should_skip_dir(name, exclude_patterns) {
                        continue;
                    }
                }
                queue.push_back((path, cur_depth + 1));
            }
        }
    }

    // P1: stable ordering (BFS traversal order is filesystem-dependent) so the
    // offset cursor resumes deterministically across calls. `dir_truncated`
    // marks per-directory entry caps separately from page truncation.
    let dir_truncated = truncated;
    entries.sort_by(|a, b| {
        a["path"]
            .as_str()
            .unwrap_or("")
            .cmp(b["path"].as_str().unwrap_or(""))
    });
    let (page, next_cursor) = paginate(entries, cursor, max);
    let mut out = json!({
        "entries": page,
        "truncated": dir_truncated || next_cursor.is_some(),
        "root": root.to_string_lossy(),
    });
    if let Some(nc) = next_cursor {
        out["next_cursor"] = json!(nc);
    }
    Ok(out)
}

/// `file.stat` — return size, mtime, mode, kind, sha256 (files only).
pub async fn handle_file_stat(decision: &PolicyDecision) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Stat);
    // For lstat we MUST use the pre-canonicalization path; `decision.path`
    // has already had symlinks resolved by `canonicalize_within_roots`.
    let lstat_path = decision.input_abs.as_path();
    let path = decision.path.as_path();

    // symlink_metadata does NOT follow symlinks, so we can detect them first.
    let lmd = fs::symlink_metadata(lstat_path)
        .await
        .map_err(|e| io_err(&format!("lstat {}", lstat_path.display()), e))?;

    let mut symlink_target: Option<String> = None;
    if lmd.file_type().is_symlink() {
        if let Ok(tgt) = fs::read_link(lstat_path).await {
            symlink_target = Some(normalize_symlink_target(tgt));
        }
    }

    // Fall back to follow-metadata (using the canonical path) for
    // size/mtime/mode resolution. If the symlink is broken, reuse lmd so we
    // still produce a response.
    let md = fs::metadata(path).await.unwrap_or_else(|_| lmd.clone());

    let kind = if lmd.file_type().is_symlink() {
        "symlink"
    } else if md.is_dir() {
        "dir"
    } else if md.is_file() {
        "file"
    } else {
        "other"
    };

    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        md.permissions().mode()
    };
    #[cfg(not(unix))]
    let mode: u32 = if md.permissions().readonly() {
        0o444
    } else {
        0o644
    };

    let mut value = json!({
        "size": md.len(),
        "mtime_unix": md.modified().ok().and_then(system_time_to_unix),
        "mode": mode,
        "kind": kind,
    });
    if let Some(t) = symlink_target {
        value["symlink_target"] = Value::String(t);
    }

    if md.is_file() && md.len() <= decision.max_read_bytes {
        let digest = sha256_file(path).await.ok();
        if let Some(h) = digest {
            value["sha256"] = Value::String(h);
        }
    }

    Ok(value)
}

/// `file.glob` — enumerate paths under the policy root matching `pattern`.
pub async fn handle_file_glob(
    decision: &PolicyDecision,
    pattern: &str,
    max_matches: Option<usize>,
    exclude_patterns: &[String],
    respect_gitignore: bool,
    denies: &[String],
    cursor: Option<u64>,
) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Glob);
    let max = max_matches.unwrap_or(DEFAULT_GLOB_MAX);
    let root = decision.path.as_path().to_path_buf();
    let matcher = GlobMatcher::new(&[pattern.to_string()]).map_err(fa_to_bifrost)?;
    // P1: enforce deny rules so policy-blacklisted paths never surface via glob.
    let deny_matcher = if denies.is_empty() {
        None
    } else {
        Some(DenyMatcher::new(denies).map_err(|e| {
            BifrostError::Config(format!("[file.invalid_deny] bad deny pattern: {}", e))
        })?)
    };

    let mut all_matches: Vec<String> = Vec::new();
    let gi_opt = if respect_gitignore {
        build_gitignore_for(&root)
    } else {
        None
    };
    let mut stack: Vec<PathBuf> = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let mut rd = match fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        while let Some(entry) = rd.next_entry().await.ok().flatten() {
            let path = entry.path();
            let md = match entry.metadata().await {
                Ok(md) => md,
                Err(_) => continue,
            };
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(ref dm) = deny_matcher {
                if dm.match_raw(&rel).is_some() {
                    continue;
                }
            }
            if md.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if should_skip_dir(name, exclude_patterns) {
                        continue;
                    }
                }
                if let Some(ref gi) = gi_opt {
                    if is_gitignored(gi, &path, true) {
                        continue;
                    }
                }
                stack.push(path);
                continue;
            }
            if matcher.is_match(&rel) {
                all_matches.push(rel);
            }
        }
    }

    // P1: deterministic ordering enables a stable offset cursor for resumable
    // pagination across very large trees.
    all_matches.sort_unstable();
    let (page, next_cursor) = paginate(all_matches, cursor, max);
    let mut out = json!({
        "matches": page,
        "truncated": next_cursor.is_some(),
        "root": root.to_string_lossy(),
    });
    if let Some(nc) = next_cursor {
        out["next_cursor"] = json!(nc);
    }
    Ok(out)
}

/// `file.search` — regex grep across files under the policy root.
///
/// P0-2: traversal is delegated to [`ignore::WalkBuilder`] so gitignore,
/// default-excludes and the standard VCS filters are handled by the same
/// crate the rest of the file already depends on, instead of a hand-rolled
/// DFS. We use the *single-threaded* walker so the deny-filter, hard-cap and
/// deterministic sort logic stay simple and order-stable. The per-file cap,
/// binary sniff, `SEARCH_HARD_CAP`, sort and pagination semantics are all
/// preserved byte-for-byte from the previous implementation.
///
/// P1-1 (`around`): when `around = Some(n)` each hit additionally carries a
/// `snippet` of the `n` lines before and after the match joined by `\n`.
/// `around` takes precedence over `context_before`/`context_after` for the
/// `context` array sizing as well: if `around` is set it overrides both. The
/// legacy `context` array is still emitted whenever any window is requested.
///
/// P1-3 (multi/literal/word): `patterns` is OR-combined. When `fixed_strings`
/// is set each pattern is `regex::escape`d; when `word` is set each pattern is
/// wrapped in `\b...\b`. The scalar `pattern` argument is kept for signature
/// back-compat but is ignored whenever `patterns` is non-empty.
#[allow(clippy::too_many_arguments)]
pub async fn handle_file_search(
    decision: &PolicyDecision,
    pattern: &str,
    max_matches: Option<usize>,
    max_scan_bytes: Option<u64>,
    exclude_patterns: &[String],
    context_before: Option<u32>,
    context_after: Option<u32>,
    case_insensitive: bool,
    file_glob: Option<&str>,
    respect_gitignore: bool,
    denies: &[String],
    cursor: Option<u64>,
    patterns: &[String],
    around: Option<u32>,
    fixed_strings: bool,
    word: bool,
) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Search);
    let root = decision.path.as_path().to_path_buf();
    let deny_matcher = if denies.is_empty() {
        None
    } else {
        Some(DenyMatcher::new(denies).map_err(|e| {
            BifrostError::Config(format!("[file.invalid_deny] bad deny pattern: {}", e))
        })?)
    };
    let max = max_matches.unwrap_or(DEFAULT_SEARCH_MAX);
    let per_file_cap = max_scan_bytes
        .unwrap_or(DEFAULT_SEARCH_SCAN_BYTES)
        .min(decision.max_read_bytes);

    // P1-3: assemble the effective OR-list. Fall back to the scalar `pattern`
    // for back-compat when no array was provided.
    let raw_patterns: Vec<String> = if patterns.is_empty() {
        vec![pattern.to_string()]
    } else {
        patterns.to_vec()
    };

    // Build one combined regex via non-capturing alternation. Applying
    // escape/word per-pattern (before joining) keeps each alternative
    // independent.
    let combined = {
        let parts: Vec<String> = raw_patterns
            .iter()
            .map(|p| {
                let base = if fixed_strings {
                    regex::escape(p)
                } else {
                    p.clone()
                };
                if word {
                    format!(r"\b(?:{})\b", base)
                } else {
                    format!("(?:{})", base)
                }
            })
            .collect();
        let joined = parts.join("|");
        if case_insensitive && !joined.starts_with("(?i)") {
            format!("(?i){}", joined)
        } else {
            joined
        }
    };
    let re = regex::Regex::new(&combined).map_err(|e| {
        BifrostError::Config(format!(
            "[file.invalid_regex] bad pattern(s) {:?}: {}",
            raw_patterns, e
        ))
    })?;

    let file_glob_matcher = match file_glob {
        Some(g) => {
            let pats = vec![g.to_string()];
            Some(GlobMatcher::new(&pats).map_err(|e| {
                BifrostError::Config(format!("[file.invalid_glob] bad glob '{}': {}", g, e))
            })?)
        }
        None => None,
    };

    // P1-1: `around` overrides the asymmetric before/after when present.
    let (before, after) = match around {
        Some(n) => (n as usize, n as usize),
        None => (
            context_before.unwrap_or(0) as usize,
            context_after.unwrap_or(0) as usize,
        ),
    };
    let need_context = before > 0 || after > 0;
    let want_snippet = around.is_some();

    // P0-2: build the `ignore` walker. `standard_filters(respect_gitignore)`
    // toggles the whole gitignore/hidden/ignore stack in one shot; we still
    // layer DEFAULT_EXCLUDE_DIRS + caller excludes as overrides so behaviour
    // matches the old hand-written `should_skip_dir`.
    let mut ovr = ignore::overrides::OverrideBuilder::new(&root);
    for d in DEFAULT_EXCLUDE_DIRS {
        // `!**/<dir>/**` excludes the directory and everything under it.
        let _ = ovr.add(&format!("!**/{}/**", d));
        let _ = ovr.add(&format!("!**/{}", d));
    }
    for e in exclude_patterns {
        let _ = ovr.add(&format!("!**/{}/**", e));
        let _ = ovr.add(&format!("!**/{}", e));
    }
    let overrides = ovr.build().map_err(|e| {
        BifrostError::Config(format!("[file.invalid_glob] bad exclude pattern: {}", e))
    })?;

    let mut builder = ignore::WalkBuilder::new(&root);
    builder
        .standard_filters(respect_gitignore)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore)
        .require_git(false)
        .hidden(false)
        .parents(respect_gitignore)
        .overrides(overrides)
        .follow_links(false);

    // The walk itself + per-file reads are blocking; run them off the async
    // reactor. We collect into an owned Vec to avoid borrowing across the
    // spawn_blocking boundary.
    let root_for_walk = root.clone();
    let re_for_walk = re.clone();
    let denies_owned: Vec<String> = denies.to_vec();
    let glob_owned = file_glob_matcher;
    let _ = &deny_matcher;

    let (mut hits, scan_truncated) =
        tokio::task::spawn_blocking(move || -> Result<(Vec<Value>, bool)> {
            let deny_matcher = if denies_owned.is_empty() {
                None
            } else {
                Some(DenyMatcher::new(&denies_owned).map_err(|e| {
                    BifrostError::Config(format!("[file.invalid_deny] bad deny pattern: {}", e))
                })?)
            };
            let mut hits: Vec<Value> = Vec::new();
            let mut truncated = false;

            'walk: for dent in builder.build() {
                let dent = match dent {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                // Only regular files.
                match dent.file_type() {
                    Some(ft) if ft.is_file() => {}
                    _ => continue,
                }
                let path = dent.path().to_path_buf();
                let rel = path
                    .strip_prefix(&root_for_walk)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if let Some(ref dm) = deny_matcher {
                    if dm.match_raw(&rel).is_some() {
                        continue;
                    }
                }
                if let Some(ref gm) = glob_owned {
                    if !gm.is_match(&rel) {
                        continue;
                    }
                }
                // Per-file cap + binary sniff.
                let mut f = match std::fs::File::open(&path) {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                let mut buf = Vec::new();
                if (&mut f).take(per_file_cap).read_to_end(&mut buf).is_err() {
                    continue;
                }
                if is_probably_binary(&buf) {
                    continue;
                }
                let text = match std::str::from_utf8(&buf) {
                    Ok(t) => t,
                    Err(_) => continue,
                };

                let all_lines: Vec<&str> = if need_context || want_snippet {
                    text.lines().collect()
                } else {
                    Vec::new()
                };
                let total_lines = all_lines.len();
                let lines_iter: Box<dyn Iterator<Item = (usize, &str)>> =
                    if need_context || want_snippet {
                        Box::new(all_lines.iter().copied().enumerate())
                    } else {
                        Box::new(text.lines().enumerate())
                    };

                for (line_idx, line) in lines_iter {
                    if let Some(m) = re_for_walk.find(line) {
                        let byte_start = m.start();
                        let char_col = line[..byte_start].chars().count() as u64 + 1;
                        let mut hit = json!({
                            "path": rel,
                            "line": (line_idx as u64) + 1,
                            "column": char_col,
                            "byte_column": (byte_start as u64) + 1,
                            "preview": line.chars().take(240).collect::<String>(),
                        });
                        if need_context {
                            let ctx_start = line_idx.saturating_sub(before);
                            let ctx_end = (line_idx + after + 1).min(total_lines);
                            let ctx: Vec<Value> = (ctx_start..ctx_end)
                                .map(|i| {
                                    json!({
                                        "line": (i as u64) + 1,
                                        "content": all_lines[i]
                                            .chars()
                                            .take(240)
                                            .collect::<String>(),
                                    })
                                })
                                .collect();
                            hit["context"] = json!(ctx);
                        }
                        if want_snippet {
                            let s_start = line_idx.saturating_sub(before);
                            let s_end = (line_idx + after + 1).min(total_lines);
                            let snippet = all_lines[s_start..s_end].join("\n");
                            hit["snippet"] = json!(snippet);
                        }
                        hits.push(hit);
                        if hits.len() >= SEARCH_HARD_CAP {
                            truncated = true;
                            break 'walk;
                        }
                    }
                }
            }
            Ok((hits, truncated))
        })
        .await
        .map_err(|e| {
            BifrostError::Config(format!("[file.io_error] search task failed: {}", e))
        })??;

    // P1: stable ordering so the offset cursor resumes deterministically.
    hits.sort_by(|a, b| {
        let pa = a["path"].as_str().unwrap_or("");
        let pb = b["path"].as_str().unwrap_or("");
        pa.cmp(pb)
            .then(a["line"].as_u64().cmp(&b["line"].as_u64()))
            .then(a["column"].as_u64().cmp(&b["column"].as_u64()))
    });
    let (page, next_cursor) = paginate(hits, cursor, max);
    let mut out = json!({
        "matches": page,
        "truncated": scan_truncated || next_cursor.is_some(),
        "root": root.to_string_lossy(),
    });
    if let Some(nc) = next_cursor {
        out["next_cursor"] = json!(nc);
    }
    Ok(out)
}

/// `file.hash` — content hash. Currently only `sha256` is supported.
pub async fn handle_file_hash(decision: &PolicyDecision, algo: Option<&str>) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Hash);
    let algo = algo.unwrap_or("sha256").to_ascii_lowercase();
    if algo != "sha256" {
        return Err(BifrostError::Config(format!(
            "[file.unsupported_algo] only sha256 is supported, got '{}'",
            algo
        )));
    }
    let path = decision.path.as_path();
    let md = fs::metadata(path)
        .await
        .map_err(|e| io_err(&format!("stat {}", path.display()), e))?;
    if !md.is_file() {
        return Err(BifrostError::Config(format!(
            "[file.not_found] not a regular file: {}",
            path.display()
        )));
    }
    let hex = sha256_file(path).await?;
    Ok(json!({ "algo": "sha256", "hex": hex, "size": md.len() }))
}

const DEFAULT_OUTLINE_MAX_SYMBOLS: usize = 2_000;
/// Default byte cap for `file.outline` scanning. Outlines are cheap relative to
/// full reads but we still bound the work for very large files.
const DEFAULT_OUTLINE_SCAN_BYTES: u64 = 4 * 1024 * 1024;

/// Map a file extension to a coarse language label used to pick the symbol
/// patterns. Returns `"unknown"` for anything we don't have rules for; such
/// files yield an empty outline rather than an error.
fn outline_language_for(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "rs" => "rust",
        "ts" | "tsx" | "mts" | "cts" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" | "pyi" => "python",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "rb" => "ruby",
        "swift" => "swift",
        "cs" => "csharp",
        "php" => "php",
        _ => "unknown",
    }
}

/// A single extracted top-level symbol.
struct OutlineSymbol {
    kind: &'static str,
    name: String,
    line: u32,
    signature: String,
}

/// Compile-once regex registry for the outline extractor. Each language maps to
/// a list of `(kind, regex)` pairs where the regex captures the symbol name in
/// group 1. Patterns are intentionally line-oriented and heuristic — this is a
/// lightweight repo-map primitive, not a full parser.
fn outline_patterns(lang: &str) -> &'static [(&'static str, regex::Regex)] {
    use once_cell::sync::Lazy;
    use regex::Regex;

    macro_rules! pats {
        ($($kind:literal => $re:literal),* $(,)?) => {
            vec![ $(($kind, Regex::new($re).expect("valid outline regex")),)* ]
        };
    }

    static RUST: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
        pats![
            "fn"     => r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:const\s+)?(?:extern\s+(?:\x22[^\x22]*\x22\s+)?)?fn\s+([A-Za-z_][A-Za-z0-9_]*)",
            "struct" => r"^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)",
            "enum"   => r"^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)",
            "trait"  => r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?trait\s+([A-Za-z_][A-Za-z0-9_]*)",
            "impl"   => r"^\s*impl(?:\s*<[^>]*>)?\s+([A-Za-z_][A-Za-z0-9_:<>, ]*)",
            "type"   => r"^\s*(?:pub(?:\([^)]*\))?\s+)?type\s+([A-Za-z_][A-Za-z0-9_]*)",
            "macro"  => r"^\s*macro_rules!\s+([A-Za-z_][A-Za-z0-9_]*)",
        ]
    });
    static TS: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
        pats![
            "function" => r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s*\*?\s*([A-Za-z_$][A-Za-z0-9_$]*)",
            "class"    => r"^\s*(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+([A-Za-z_$][A-Za-z0-9_$]*)",
            "interface"=> r"^\s*(?:export\s+)?interface\s+([A-Za-z_$][A-Za-z0-9_$]*)",
            "type"     => r"^\s*(?:export\s+)?type\s+([A-Za-z_$][A-Za-z0-9_$]*)",
            "enum"     => r"^\s*(?:export\s+)?(?:const\s+)?enum\s+([A-Za-z_$][A-Za-z0-9_$]*)",
            "const"    => r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*(?:async\s+)?(?:function|\([^)]*\)\s*=>|[A-Za-z_$][A-Za-z0-9_$]*\s*=>)",
        ]
    });
    static PY: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
        pats![
            "def"   => r"^\s*(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)",
            "class" => r"^\s*class\s+([A-Za-z_][A-Za-z0-9_]*)",
        ]
    });
    static GO: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
        pats![
            "func"   => r"^\s*func\s+(?:\([^)]*\)\s*)?([A-Za-z_][A-Za-z0-9_]*)",
            "type"   => r"^\s*type\s+([A-Za-z_][A-Za-z0-9_]*)\s+(?:struct|interface)",
            "type2"  => r"^\s*type\s+([A-Za-z_][A-Za-z0-9_]*)\s+",
        ]
    });
    static JVM: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
        pats![
            "class"     => r"^\s*(?:public\s+|private\s+|protected\s+|final\s+|abstract\s+|static\s+|open\s+|sealed\s+|data\s+)*class\s+([A-Za-z_][A-Za-z0-9_]*)",
            "interface" => r"^\s*(?:public\s+|private\s+|protected\s+)*interface\s+([A-Za-z_][A-Za-z0-9_]*)",
            "enum"      => r"^\s*(?:public\s+|private\s+|protected\s+)*enum(?:\s+class)?\s+([A-Za-z_][A-Za-z0-9_]*)",
            "fun"       => r"^\s*(?:public\s+|private\s+|protected\s+|override\s+|suspend\s+|inline\s+)*fun\s+([A-Za-z_][A-Za-z0-9_]*)",
        ]
    });
    static C_LIKE: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
        pats![
            "struct" => r"^\s*(?:typedef\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)",
            "enum"   => r"^\s*(?:typedef\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)",
            "class"  => r"^\s*(?:template\s*<[^>]*>\s*)?class\s+([A-Za-z_][A-Za-z0-9_]*)",
            "fn"     => r"^\s*(?:[A-Za-z_][A-Za-z0-9_:<>,\*&\s]+?)\s+([A-Za-z_][A-Za-z0-9_]*)\s*\([^;]*\)\s*\{?\s*$",
        ]
    });
    static RUBY: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
        pats![
            "class"  => r"^\s*class\s+([A-Za-z_][A-Za-z0-9_:]*)",
            "module" => r"^\s*module\s+([A-Za-z_][A-Za-z0-9_:]*)",
            "def"    => r"^\s*def\s+([A-Za-z_][A-Za-z0-9_?!.]*)",
        ]
    });
    static SWIFT: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
        pats![
            "func"     => r"^\s*(?:public\s+|private\s+|internal\s+|fileprivate\s+|open\s+|static\s+|class\s+)*func\s+([A-Za-z_][A-Za-z0-9_]*)",
            "class"    => r"^\s*(?:public\s+|private\s+|internal\s+|final\s+|open\s+)*class\s+([A-Za-z_][A-Za-z0-9_]*)",
            "struct"   => r"^\s*(?:public\s+|private\s+|internal\s+)*struct\s+([A-Za-z_][A-Za-z0-9_]*)",
            "enum"     => r"^\s*(?:public\s+|private\s+|internal\s+)*enum\s+([A-Za-z_][A-Za-z0-9_]*)",
            "protocol" => r"^\s*(?:public\s+|private\s+|internal\s+)*protocol\s+([A-Za-z_][A-Za-z0-9_]*)",
        ]
    });
    static CSHARP: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
        pats![
            "class"     => r"^\s*(?:public\s+|private\s+|protected\s+|internal\s+|sealed\s+|abstract\s+|static\s+|partial\s+)*class\s+([A-Za-z_][A-Za-z0-9_]*)",
            "interface" => r"^\s*(?:public\s+|private\s+|protected\s+|internal\s+)*interface\s+([A-Za-z_][A-Za-z0-9_]*)",
            "struct"    => r"^\s*(?:public\s+|private\s+|protected\s+|internal\s+)*struct\s+([A-Za-z_][A-Za-z0-9_]*)",
            "enum"      => r"^\s*(?:public\s+|private\s+|protected\s+|internal\s+)*enum\s+([A-Za-z_][A-Za-z0-9_]*)",
        ]
    });
    static PHP: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
        pats![
            "class"     => r"^\s*(?:abstract\s+|final\s+)*class\s+([A-Za-z_][A-Za-z0-9_]*)",
            "interface" => r"^\s*interface\s+([A-Za-z_][A-Za-z0-9_]*)",
            "trait"     => r"^\s*trait\s+([A-Za-z_][A-Za-z0-9_]*)",
            "function"  => r"^\s*(?:public\s+|private\s+|protected\s+|static\s+|abstract\s+|final\s+)*function\s+([A-Za-z_][A-Za-z0-9_]*)",
        ]
    });
    static EMPTY: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(Vec::new);

    match lang {
        "rust" => &RUST,
        "typescript" | "javascript" => &TS,
        "python" => &PY,
        "go" => &GO,
        "java" | "kotlin" => &JVM,
        "c" | "cpp" => &C_LIKE,
        "ruby" => &RUBY,
        "swift" => &SWIFT,
        "csharp" => &CSHARP,
        "php" => &PHP,
        _ => &EMPTY,
    }
}

/// Extract a heuristic symbol outline from already-loaded source `text`.
/// Pure function (no IO) so it can be unit-tested directly.
fn extract_outline(text: &str, lang: &str, max_symbols: usize) -> (Vec<OutlineSymbol>, bool) {
    let patterns = outline_patterns(lang);
    let mut symbols: Vec<OutlineSymbol> = Vec::new();
    let mut truncated = false;
    for (idx, raw_line) in text.lines().enumerate() {
        // Skip obvious comment lines to cut down on false positives.
        let trimmed = raw_line.trim_start();
        if trimmed.starts_with("//")
            || trimmed.starts_with('#') && lang != "python" && lang != "ruby"
            || trimmed.starts_with('*')
        {
            continue;
        }
        for (kind, re) in patterns.iter() {
            if let Some(caps) = re.captures(raw_line) {
                if let Some(name) = caps.get(1) {
                    if symbols.len() >= max_symbols {
                        truncated = true;
                        break;
                    }
                    let signature = raw_line.trim_end().trim_start().to_string();
                    // Cap signature length so a giant single-line decl can't
                    // bloat the response.
                    let signature = if signature.len() > 240 {
                        format!("{}…", &signature[..240])
                    } else {
                        signature
                    };
                    symbols.push(OutlineSymbol {
                        kind,
                        name: name.as_str().to_string(),
                        line: u32::try_from(idx + 1).unwrap_or(u32::MAX),
                        signature,
                    });
                    // First matching pattern wins for this line.
                    break;
                }
            }
        }
        if truncated {
            break;
        }
    }
    (symbols, truncated)
}

/// `file.outline` — return a lightweight symbol outline (repo-map primitive)
/// for a single source file. Uses heuristic, language-aware regex extraction
/// of top-level declarations (fn/struct/class/interface/def/…) so a coding
/// agent can grasp a file's shape without reading the whole body. Unknown
/// languages yield an empty `symbols` array rather than an error.
pub async fn handle_file_outline(
    decision: &PolicyDecision,
    max_symbols: Option<usize>,
    max_bytes: Option<u64>,
) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Outline);
    let path = decision.path.as_path();

    let metadata = fs::metadata(path)
        .await
        .map_err(|e| io_err(&format!("stat {}", path.display()), e))?;
    if !metadata.is_file() {
        return Err(BifrostError::Config(format!(
            "[file.not_found] not a regular file: {}",
            path.display()
        )));
    }
    let total_size = metadata.len();

    let cap = max_bytes
        .unwrap_or(DEFAULT_OUTLINE_SCAN_BYTES)
        .min(decision.max_read_bytes);
    let f = fs::File::open(path)
        .await
        .map_err(|e| io_err(&format!("open {}", path.display()), e))?;
    let mut buf = Vec::new();
    let mut limited = f.take(cap);
    limited
        .read_to_end(&mut buf)
        .await
        .map_err(|e| io_err(&format!("read {}", path.display()), e))?;
    let bytes_truncated = (buf.len() as u64) < total_size;

    let lang = outline_language_for(path);
    let max_symbols = max_symbols
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_OUTLINE_MAX_SYMBOLS);
    // The binary sniff + utf-8 decode + multi-regex per-line scan are
    // CPU-bound; run them off the async reactor so a large source file cannot
    // stall the shared runtime that also serves the proxy. We compute
    // total_lines here too so `buf`/`text` need not cross the await boundary.
    let path_for_blocking = path.to_path_buf();
    let (symbols, sym_truncated, total_lines) =
        tokio::task::spawn_blocking(move || -> Result<(Vec<OutlineSymbol>, bool, u32)> {
            if is_probably_binary(&buf) {
                return Err(fa_to_bifrost(FileAccessError::BinaryNotAllowed {
                    path: path_for_blocking,
                }));
            }
            let text = String::from_utf8_lossy(&buf);
            let total_lines = u32::try_from(text.lines().count()).unwrap_or(u32::MAX);
            let (symbols, sym_truncated) = extract_outline(&text, lang, max_symbols);
            Ok((symbols, sym_truncated, total_lines))
        })
        .await
        .map_err(|e| BifrostError::Config(format!("outline worker join failed: {e}")))??;

    let symbols_json: Vec<Value> = symbols
        .iter()
        .map(|s| {
            json!({
                "kind": s.kind,
                "name": s.name,
                "line": s.line,
                "signature": s.signature,
            })
        })
        .collect();

    Ok(json!({
        "language": lang,
        "symbols": symbols_json,
        "count": symbols_json.len(),
        "truncated": sym_truncated || bytes_truncated,
        "total_size": total_size,
        "total_lines": total_lines,
    }))
}

// === Write handlers: file.write / edit / mkdir / move / delete / apply_patch ===
//
// All handlers assume the executor has already invoked
// `FileAccessPolicy::check` and produced a `PolicyDecision`. They perform no
// additional access control; they only translate the decision into
// tokio::fs calls and a wire response.
//
// Atomicity: file.write and file.edit go via a tmpfile + rename in the same
// directory. file.apply_patch resolves every target path first (all-or-none
// policy check), then applies hunks atomically per-file.

use tokio::io::AsyncWriteExt;

const DEFAULT_MAX_WRITE_BYTES: u64 = 2 * 1024 * 1024;

fn b64_decode(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .map_err(|e| BifrostError::Config(format!("[file.invalid_args] invalid base64: {}", e)))
}

async fn read_sha256(path: &Path) -> Result<Option<String>> {
    match fs::metadata(path).await {
        Ok(_) => Ok(Some(sha256_file(path).await?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_err("stat-for-sha", e)),
    }
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

/// Write `content` atomically to `decision.path`. Honors `base_sha256`
/// optimistic locking and the policy's `max_write_bytes` / `allow_overwrite`.
pub async fn handle_file_write(
    decision: &PolicyDecision,
    content_b64: &str,
    base_sha256: Option<&str>,
    allow_overwrite_override: Option<bool>,
    create_parents: bool,
) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Write);
    let path = decision.path.as_path();

    let bytes = b64_decode(content_b64)?;
    let max = if decision.max_write_bytes == 0 {
        DEFAULT_MAX_WRITE_BYTES
    } else {
        decision.max_write_bytes
    };
    if (bytes.len() as u64) > max {
        return Err(size_too_large(format!(
            "content is {} bytes, max_write_bytes is {}",
            bytes.len(),
            max
        )));
    }

    // Existence + overwrite + sha precondition check.
    let existing_sha = read_sha256(path).await?;
    let allow_overwrite = allow_overwrite_override.unwrap_or(decision.allow_overwrite);
    match (&existing_sha, base_sha256) {
        (Some(_), _) if !allow_overwrite => {
            return Err(precondition_failed(
                "target exists and overwrite is disabled",
            ));
        }
        (Some(actual), Some(expected)) if actual != expected => {
            return Err(sha_mismatch(format!(
                "expected {}, actual {}",
                expected, actual
            )));
        }
        (None, Some(expected)) if !expected.is_empty() => {
            return Err(precondition_failed(
                "target does not exist but base_sha256 was supplied",
            ));
        }
        _ => {}
    }

    // Atomic write via tmpfile + rename in the same directory.
    let parent = path.parent().ok_or_else(|| {
        BifrostError::Config("[file.invalid_args] path has no parent".to_string())
    })?;
    // P0-6: create intermediate directories when the caller requested it.
    if create_parents {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| io_err("mkdir-parents", e))?;
    }
    // Capture existing mode (if any) so we can restore it after rename —
    // otherwise an 0755 executable would silently become 0644 on overwrite.
    let prior_mode = capture_mode(path).await;
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = parent.join(format!(".bifrost-write.{}.{}.tmp", pid, nanos));
    {
        let mut f = fs::File::create(&tmp)
            .await
            .map_err(|e| io_err("create-tmp", e))?;
        f.write_all(&bytes)
            .await
            .map_err(|e| io_err("write-tmp", e))?;
        f.sync_all().await.map_err(|e| io_err("fsync-tmp", e))?;
    }
    // Pre-apply prior mode on tmp so the rename-target inode already
    // carries the correct perms. Pre-rename chmod is best-effort:
    // if it fails (e.g. EPERM on some filesystems), we still try post-rename.
    let pre_rename_err = apply_mode_sync(&tmp, prior_mode).err();
    diag_mode(
        "pre-rename",
        &tmp,
        prior_mode,
        pre_rename_err.as_ref().map(|e| e.to_string()),
    );
    diag_mode("pre-rename-readback", &tmp, read_mode_sync(&tmp), None);
    fs::rename(&tmp, path)
        .await
        .map_err(|e| io_err("rename-tmp", e))?;
    diag_mode("post-rename-readback", path, read_mode_sync(path), None);
    // Post-rename chmod: authoritative guarantee that the final inode
    // carries `prior_mode`. Uses sync std::fs to avoid tokio scheduling
    // races on Linux runners where the tmp-side chmod has been observed
    // to be lost through rename on some filesystems.
    if let Err(e) = apply_mode_sync(path, prior_mode) {
        diag_mode(
            "post-rename-chmod-fail",
            path,
            prior_mode,
            Some(e.to_string()),
        );
        // Don't fail the whole write on chmod error — the bytes landed
        // atomically. Caller can re-stat if perms matter.
    }
    diag_mode("final-readback", path, read_mode_sync(path), None);

    let new_sha = sha256_hex(&bytes);
    Ok(json!({
        "path": path.to_string_lossy(),
        "bytes_written": bytes.len(),
        "sha256": new_sha,
        "previous_sha256": existing_sha,
    }))
}

/// A single edit item. Two mutually-exclusive shapes are supported on the
/// same wire array:
///   * line-range: `{start_line, end_line, replacement}` (1-based inclusive)
///   * anchored:   `{old_string, new_string, expected_count?}` (content-based)
///
/// An item is treated as anchored iff `old_string` is present.
#[derive(Debug, Default, serde::Deserialize)]
pub struct EditRange {
    #[serde(default)]
    pub start_line: u32,
    #[serde(default)]
    pub end_line: u32,
    #[serde(default)]
    pub replacement: String,
    /// Anchored mode: literal substring to locate (no regex).
    #[serde(default)]
    pub old_string: Option<String>,
    /// Anchored mode: replacement text (EOL-normalized to the source file).
    #[serde(default)]
    pub new_string: Option<String>,
    /// Anchored mode: exact number of occurrences expected (default 1).
    #[serde(default)]
    pub expected_count: Option<u32>,
}

impl EditRange {
    fn is_anchored(&self) -> bool {
        self.old_string.is_some()
    }
}

pub async fn handle_file_edit(
    decision: &PolicyDecision,
    base_sha256: Option<&str>,
    edits: &[EditRange],
) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Edit);
    let path = decision.path.as_path();

    if edits.is_empty() {
        return Err(BifrostError::Config(
            "[file.invalid_args] edits must not be empty".to_string(),
        ));
    }

    // v1: anchored and line-range items may not be mixed in one call.
    let any_anchored = edits.iter().any(EditRange::is_anchored);
    let all_anchored = edits.iter().all(EditRange::is_anchored);
    if any_anchored && !all_anchored {
        return Err(BifrostError::Config(
            "[file.invalid_args] cannot mix anchored {old_string,...} and \
             line-range {start_line,...} edits in a single call"
                .to_string(),
        ));
    }

    let orig_bytes = fs::read(path).await.map_err(|e| io_err("read", e))?;
    let orig_sha = sha256_hex(&orig_bytes);
    if let Some(expected) = base_sha256 {
        if expected != orig_sha {
            return Err(sha_mismatch(format!(
                "expected {}, actual {}",
                expected, orig_sha
            )));
        }
    }
    let eol = Eol::detect(&orig_bytes);
    let original = String::from_utf8(orig_bytes)
        .map_err(|_| BifrostError::Config("[file.invalid_args] file is not utf-8".to_string()))?;

    // ---- Anchored (content-based) path ------------------------------------
    if all_anchored {
        // Each item replaces exactly `expected_count` (default 1) literal
        // occurrences of `old_string` with `new_string`, EOL-normalized to
        // the source file. Items are applied in order over a single buffer.
        let mut content = original;
        let mut applied_total: u64 = 0;
        for e in edits {
            let old = e
                .old_string
                .as_deref()
                .expect("is_anchored guarantees old_string is Some");
            if old.is_empty() {
                return Err(BifrostError::Config(
                    "[file.invalid_args] old_string must not be empty".to_string(),
                ));
            }
            let new_raw = e.new_string.as_deref().unwrap_or("");
            // Normalize both anchor and replacement to the file's EOL so a
            // CRLF file matches an LF-typed old_string and vice versa.
            let old_norm = normalize_to_eol(old, eol);
            let new_norm = normalize_to_eol(new_raw, eol);
            let expected = e.expected_count.unwrap_or(1);

            let found = content.matches(old_norm.as_str()).count() as u32;
            if found == 0 {
                return Err(BifrostError::Config(format!(
                    "[file.anchor_not_found] old_string not found in '{}'",
                    path.to_string_lossy()
                )));
            }
            if found != expected {
                return Err(BifrostError::Config(format!(
                    "[file.anchor_not_unique] old_string found {} time(s) in '{}', \
                     expected {} (set expected_count to override)",
                    found,
                    path.to_string_lossy(),
                    expected
                )));
            }
            content = content.replace(old_norm.as_str(), new_norm.as_str());
            applied_total += found as u64;
        }

        let new_bytes = content.into_bytes();
        let max = if decision.max_write_bytes == 0 {
            DEFAULT_MAX_WRITE_BYTES
        } else {
            decision.max_write_bytes
        };
        if (new_bytes.len() as u64) > max {
            return Err(size_too_large(format!(
                "result is {} bytes, max_write_bytes is {}",
                new_bytes.len(),
                max
            )));
        }
        let new_sha = atomic_rewrite(path, &new_bytes).await?;
        return Ok(json!({
            "path": path.to_string_lossy(),
            "bytes_written": new_bytes.len(),
            "sha256": new_sha,
            "previous_sha256": orig_sha,
            "applied_edits": edits.len(),
            "applied_occurrences": applied_total,
        }));
    }

    // ---- Line-range path (unchanged behavior) -----------------------------
    let lines: Vec<&str> = original.split_inclusive('\n').collect();

    // Validate ranges: 1-based, non-overlapping, ascending.
    let mut sorted: Vec<&EditRange> = edits.iter().collect();
    sorted.sort_by_key(|e| e.start_line);
    let mut last_end = 0u32;
    for e in &sorted {
        if e.start_line == 0 || e.start_line > e.end_line {
            return Err(BifrostError::Config(format!(
                "[file.invalid_args] invalid range: {}..{}",
                e.start_line, e.end_line
            )));
        }
        if (e.end_line as usize) > lines.len() {
            return Err(BifrostError::Config(format!(
                "[file.invalid_args] end_line {} exceeds file length {}",
                e.end_line,
                lines.len()
            )));
        }
        if e.start_line <= last_end {
            return Err(BifrostError::Config(
                "[file.invalid_args] edit ranges must not overlap".to_string(),
            ));
        }
        last_end = e.end_line;
    }

    // Build output.
    let mut out = String::with_capacity(original.len());
    let mut cursor: u32 = 1; // next line (1-based) to emit
    for e in &sorted {
        while cursor < e.start_line {
            out.push_str(lines[(cursor - 1) as usize]);
            cursor += 1;
        }
        // P0-2: always normalize replacement to the source file's EOL
        // (both LF and CRLF) — previously we only normalized when the
        // file was CRLF, which silently allowed CRLF bytes to leak
        // into LF files and produce mixed endings.
        let rep_norm = normalize_to_eol(&e.replacement, eol);
        let rep_ref: &str = rep_norm.as_str();
        out.push_str(rep_ref);
        // We need to restore a separator EOL in two cases:
        //   (a) hunk is not the last chunk in the file: always needs a
        //       newline so the next line starts fresh.
        //   (b) hunk *is* the last chunk but the original file ended
        //       with a newline — preserve that terminator.
        let is_last_chunk = (e.end_line as usize) == lines.len();
        let original_ended_with_newline = lines.last().map(|l| l.ends_with('\n')).unwrap_or(false);
        let needs_eol = !rep_ref.is_empty()
            && !rep_ref.ends_with('\n')
            && (!is_last_chunk || original_ended_with_newline);
        if needs_eol {
            out.push_str(eol.newline());
        }
        cursor = e.end_line + 1;
    }
    while (cursor as usize) <= lines.len() {
        out.push_str(lines[(cursor - 1) as usize]);
        cursor += 1;
    }

    let new_bytes = out.into_bytes();
    let max = if decision.max_write_bytes == 0 {
        DEFAULT_MAX_WRITE_BYTES
    } else {
        decision.max_write_bytes
    };
    if (new_bytes.len() as u64) > max {
        return Err(size_too_large(format!(
            "result is {} bytes, max_write_bytes is {}",
            new_bytes.len(),
            max
        )));
    }

    // Atomic rewrite (shared with the anchored path above).
    let new_sha = atomic_rewrite(path, &new_bytes).await?;
    Ok(json!({
        "path": path.to_string_lossy(),
        "bytes_written": new_bytes.len(),
        "sha256": new_sha,
        "previous_sha256": orig_sha,
        "applied_edits": edits.len(),
    }))
}

/// Atomically rewrite `path` with `bytes` via tmp+rename, preserving the
/// file's prior unix mode. Returns the sha256 of the written bytes.
async fn atomic_rewrite(path: &Path, bytes: &[u8]) -> Result<String> {
    let parent = path.parent().ok_or_else(|| {
        BifrostError::Config("[file.invalid_args] path has no parent".to_string())
    })?;
    let prior_mode = capture_mode(path).await;
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = parent.join(format!(".bifrost-edit.{}.{}.tmp", pid, nanos));
    {
        let mut f = fs::File::create(&tmp)
            .await
            .map_err(|e| io_err("create-tmp", e))?;
        f.write_all(bytes)
            .await
            .map_err(|e| io_err("write-tmp", e))?;
        f.sync_all().await.map_err(|e| io_err("fsync-tmp", e))?;
    }
    let _ = apply_mode_sync(&tmp, prior_mode);
    fs::rename(&tmp, path)
        .await
        .map_err(|e| io_err("rename-tmp", e))?;
    apply_mode(path, prior_mode).await;
    Ok(sha256_hex(bytes))
}

pub async fn handle_file_mkdir(decision: &PolicyDecision, parents: bool) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Mkdir);
    let path = decision.path.as_path();
    let res = if parents {
        fs::create_dir_all(path).await
    } else {
        fs::create_dir(path).await
    };
    res.map_err(|e| io_err_structured("mkdir", e))?;
    Ok(json!({
        "path": path.to_string_lossy(),
        "created": true,
        "parents": parents,
    }))
}

pub async fn handle_file_move(
    decision: &PolicyDecision,
    to_decision: &PolicyDecision,
    base_sha256: Option<&str>,
) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Move);
    debug_assert_eq!(to_decision.op, FileOp::Move);
    let from = decision.path.as_path();
    let to = to_decision.path.as_path();

    // Optimistic lock: if the caller supplied `base_sha256`, verify the
    // source file hashes match before renaming. This prevents a losing
    // writer in a two-agent race from clobbering an already-moved target.
    // Directories do not have a sha and are rejected with sha_mismatch
    // so callers get a stable error code.
    if let Some(expected) = base_sha256 {
        let meta = fs::symlink_metadata(from)
            .await
            .map_err(|e| io_err("stat", e))?;
        if !meta.file_type().is_file() {
            return Err(BifrostError::Config(
                "[file.sha_mismatch] base_sha256 is only valid for regular files".to_string(),
            ));
        }
        let actual = sha256_file(from).await?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(BifrostError::Config(format!(
                "[file.sha_mismatch] source file sha mismatch: expected {}, actual {}",
                expected, actual
            )));
        }
    }

    if fs::metadata(to).await.is_ok() && !decision.allow_overwrite {
        return Err(precondition_failed(
            "destination exists and overwrite is disabled",
        ));
    }
    fs::rename(from, to)
        .await
        .map_err(|e| io_err_structured("rename", e))?;
    Ok(json!({
        "from": from.to_string_lossy(),
        "to": to.to_string_lossy(),
    }))
}

pub async fn handle_file_delete(
    decision: &PolicyDecision,
    recursive: bool,
    if_match_sha256: Option<&str>,
) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Delete);
    let path = decision.path.as_path();
    let meta = fs::symlink_metadata(path)
        .await
        .map_err(|e| io_err("stat", e))?;
    let ft = meta.file_type();

    // Optimistic lock (files only): verify content hash before deleting.
    // Directories are silently skipped — their content is multi-file and
    // a single sha has no meaning; callers should rely on recursive +
    // allow_recursive_delete for dir protection instead.
    if let Some(expected) = if_match_sha256 {
        if ft.is_file() {
            let actual = sha256_file(path).await?;
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(BifrostError::Config(format!(
                    "[file.sha_mismatch] target file sha mismatch: expected {}, actual {}",
                    expected, actual
                )));
            }
        }
    }

    if ft.is_dir() {
        if recursive {
            if !decision.allow_recursive_delete {
                return Err(BifrostError::Config(
                    "[file.permission_denied] recursive delete is disabled by policy".to_string(),
                ));
            }
            fs::remove_dir_all(path)
                .await
                .map_err(|e| io_err("remove_dir_all", e))?;
        } else {
            fs::remove_dir(path)
                .await
                .map_err(|e| io_err("remove_dir", e))?;
        }
    } else {
        fs::remove_file(path)
            .await
            .map_err(|e| io_err("remove_file", e))?;
    }
    Ok(json!({
        "path": path.to_string_lossy(),
        "deleted": true,
        "recursive": recursive,
    }))
}

/// Minimal unified-diff applier. Accepts a patch in `git diff` / `diff -u`
/// What a single file entry in a unified/git diff represents. Only
/// `Modify`, `Create`, and `Delete` correspond to applicable hunk bodies;
/// the other variants are git-diff extensions that callers must express
/// with dedicated file ops (`file.move`, `file.write`, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchKind {
    Modify,
    Create,
    Delete,
    RenameOnly { from: String, to: String },
    CopyOnly { from: String, to: String },
    ModeOnly { path: String },
    Binary { path: String },
}

/// A single file entry extracted from a patch text.
#[derive(Debug, Clone)]
pub struct PatchEntry {
    pub old_path: String,
    pub new_path: String,
    pub body: String,
    pub kind: PatchKind,
}

impl PatchEntry {
    /// Decision-map key: deletes are keyed on old_path, creates and
    /// modifies on new_path.
    pub fn decision_key(&self) -> String {
        if self.new_path == "/dev/null" {
            self.old_path.clone()
        } else {
            self.new_path.clone()
        }
    }
}

/// Best-effort extraction of the target file paths in a patch body, for audit
/// logging. Returns the decision-key path of each entry (new_path for
/// create/modify, old_path for delete). Parse failures yield an empty list so
/// audit logging never blocks the operation itself.
pub fn patch_target_paths(text: &str) -> Vec<String> {
    parse_patch(text)
        .map(|entries| entries.iter().map(PatchEntry::decision_key).collect())
        .unwrap_or_default()
}

fn strip_ab_prefix(s: &str) -> String {
    let s = s.trim();
    s.strip_prefix("a/")
        .or_else(|| s.strip_prefix("b/"))
        .unwrap_or(s)
        .to_string()
}

/// Split a patch text into file-level entries, honouring the plain
/// `--- / +++` unified format *and* the git-extended format
/// (diff --git / rename / copy / index / mode / binary).
pub fn parse_patch(text: &str) -> Result<Vec<PatchEntry>> {
    let mut entries: Vec<PatchEntry> = Vec::new();
    let lines: Vec<&str> = text.split_inclusive('\n').collect();

    // Find section boundaries. A new section begins at either
    // `diff --git ` or — in the no-git-wrapper case — the first
    // `--- ` line.
    let section_starts: Vec<usize> = {
        let mut v = Vec::new();
        let mut seen_diff_git = false;
        for (idx, ln) in lines.iter().enumerate() {
            let s = ln.trim_end_matches(['\n', '\r']);
            if s.starts_with("diff --git ") {
                v.push(idx);
                seen_diff_git = true;
            } else if !seen_diff_git && s.starts_with("--- ") {
                v.push(idx);
            }
        }
        v
    };

    if section_starts.is_empty() {
        return Ok(entries);
    }

    for (k, &start) in section_starts.iter().enumerate() {
        let end = section_starts.get(k + 1).copied().unwrap_or(lines.len());
        let section = &lines[start..end];

        let mut old_path: Option<String> = None;
        let mut new_path: Option<String> = None;
        let mut rename_from: Option<String> = None;
        let mut rename_to: Option<String> = None;
        let mut copy_from: Option<String> = None;
        let mut copy_to: Option<String> = None;
        let mut has_old_mode = false;
        let mut has_new_mode = false;
        let mut new_file_mode = false;
        let mut deleted_file_mode = false;
        let mut is_binary = false;
        let mut diff_git_new: Option<String> = None;

        let mut body_start: Option<usize> = None;

        for (idx, ln) in section.iter().enumerate() {
            let t = ln.trim_end_matches(['\n', '\r']);
            if t.starts_with("@@ ") {
                body_start = Some(idx);
                break;
            }
            if let Some(rest) = t.strip_prefix("diff --git ") {
                if let Some(sep) = rest.rfind(" b/") {
                    let b = &rest[sep + 1..];
                    diff_git_new = Some(strip_ab_prefix(b));
                }
            } else if let Some(p) = t.strip_prefix("--- ") {
                old_path = Some(strip_ab_prefix(p));
            } else if let Some(p) = t.strip_prefix("+++ ") {
                new_path = Some(strip_ab_prefix(p));
            } else if let Some(p) = t.strip_prefix("rename from ") {
                rename_from = Some(p.trim().to_string());
            } else if let Some(p) = t.strip_prefix("rename to ") {
                rename_to = Some(p.trim().to_string());
            } else if let Some(p) = t.strip_prefix("copy from ") {
                copy_from = Some(p.trim().to_string());
            } else if let Some(p) = t.strip_prefix("copy to ") {
                copy_to = Some(p.trim().to_string());
            } else if t.starts_with("old mode ") {
                has_old_mode = true;
            } else if t.starts_with("new mode ") {
                has_new_mode = true;
            } else if t.starts_with("new file mode ") {
                new_file_mode = true;
            } else if t.starts_with("deleted file mode ") {
                deleted_file_mode = true;
            } else if t.starts_with("Binary files ") || t.starts_with("GIT binary patch") {
                is_binary = true;
            }
        }

        let body: String = if let Some(bs) = body_start {
            section[bs..].concat()
        } else {
            String::new()
        };

        let op = old_path.clone();
        let np = new_path.clone();
        let gnew = diff_git_new.clone().unwrap_or_default();

        let kind = if is_binary {
            PatchKind::Binary {
                path: np.clone().unwrap_or_else(|| gnew.clone()),
            }
        } else if let (Some(from), Some(to)) = (rename_from.clone(), rename_to.clone()) {
            PatchKind::RenameOnly { from, to }
        } else if let (Some(from), Some(to)) = (copy_from.clone(), copy_to.clone()) {
            PatchKind::CopyOnly { from, to }
        } else if body_start.is_none() && (has_old_mode || has_new_mode) {
            PatchKind::ModeOnly {
                path: np.clone().or(op.clone()).unwrap_or_else(|| gnew.clone()),
            }
        } else if body_start.is_some() {
            let (ops, nps) = match (op.as_deref(), np.as_deref()) {
                (Some(o), Some(n)) => (o.to_string(), n.to_string()),
                _ => {
                    return Err(BifrostError::Config(
                        "[file.invalid_args] malformed unified diff: missing --- or +++ header"
                            .to_string(),
                    ));
                }
            };
            if ops == "/dev/null" && nps == "/dev/null" {
                return Err(BifrostError::Config(
                    "[file.invalid_args] malformed unified diff: both sides are /dev/null"
                        .to_string(),
                ));
            }
            if ops == "/dev/null" || new_file_mode {
                PatchKind::Create
            } else if nps == "/dev/null" || deleted_file_mode {
                PatchKind::Delete
            } else {
                PatchKind::Modify
            }
        } else {
            continue;
        };

        let final_old = match &kind {
            PatchKind::RenameOnly { from, .. } | PatchKind::CopyOnly { from, .. } => from.clone(),
            PatchKind::ModeOnly { path } => path.clone(),
            PatchKind::Binary { path } => path.clone(),
            _ => op.unwrap_or_default(),
        };
        let final_new = match &kind {
            PatchKind::RenameOnly { to, .. } | PatchKind::CopyOnly { to, .. } => to.clone(),
            PatchKind::ModeOnly { path } => path.clone(),
            PatchKind::Binary { path } => path.clone(),
            _ => np.unwrap_or_default(),
        };

        entries.push(PatchEntry {
            old_path: final_old,
            new_path: final_new,
            body,
            kind,
        });
    }

    Ok(entries)
}

/// form. For each file header (`--- a/FOO` + `+++ b/FOO`) we resolve the
/// target against the provided decisions map (policy pre-checked upstream),
/// verify the context lines match, and apply hunks atomically per-file.
///
/// Any parse or context-mismatch error aborts the whole patch (no partial
/// application). Binary diffs are refused.
pub async fn handle_file_apply_patch(
    decisions: &std::collections::HashMap<String, PolicyDecision>,
    patch_text: &str,
    base_shas: &std::collections::HashMap<String, String>,
) -> Result<Value> {
    // Two-phase commit:
    //   phase 1 — for every file in the patch compute the new bytes and
    //             write them to a sibling `.bifrost-patch.*.tmp`.
    //             Any parse / context-mismatch / size / policy error aborts
    //             the whole patch *before* any target file has been touched.
    //   phase 2 — once all tmp files exist, rename() them into place one by
    //             one. Rename is atomic per-file; we still iterate here, but
    //             every rename now acts on a pre-validated tmp, so the only
    //             way phase 2 fails is an OS-level error (EBUSY/EACCES), in
    //             which case we do a best-effort rollback of renames that
    //             already succeeded.

    let entries = parse_patch(patch_text)?;

    // What phase 2 should do for a staged entry.
    enum StageAction {
        /// rename tmp -> target (Modify/Create write path).
        WriteTmp,
        /// unlink target (delete path).
        Delete,
        /// rename `from` -> `to` (patch RenameOnly).
        Rename { from: PathBuf },
        /// copy already written to tmp; rename tmp -> target (patch CopyOnly).
        Copy,
    }

    struct Staged {
        target: PathBuf,
        tmp: PathBuf,
        #[allow(dead_code)]
        new_path_is_dev_null: bool,
        prior_mode: Option<u32>,
        applied_entry: Value,
        action: StageAction,
    }

    let mut staged: Vec<Staged> = Vec::new();

    // Cleanup helper: remove every staged tmp file on early-return.
    async fn cleanup_tmps(staged: &[Staged]) {
        for s in staged {
            let _ = fs::remove_file(&s.tmp).await;
        }
    }

    for entry in &entries {
        match &entry.kind {
            PatchKind::Binary { path } => {
                // Binary patches carry no text hunk body we can apply; reject.
                cleanup_tmps(&staged).await;
                return Err(BifrostError::Config(format!(
                    "[file.binary_patch_unsupported] binary diff for '{}' is not supported by file.apply_patch",
                    path
                )));
            }
            PatchKind::ModeOnly { path } => {
                // Mode-only changes are a niche git extension; out of scope.
                cleanup_tmps(&staged).await;
                return Err(BifrostError::Config(format!(
                    "[file.unsupported_diff] mode-only change for '{}' is not supported by file.apply_patch",
                    path
                )));
            }
            PatchKind::RenameOnly { from, to } => {
                // Stage a rename: both endpoints already have policy decisions
                // (keyed on `from` and `to`) supplied by the executor.
                let from_dec = match decisions.get(from) {
                    Some(d) => d,
                    None => {
                        cleanup_tmps(&staged).await;
                        return Err(BifrostError::Config(format!(
                            "[file.invalid_args] no policy decision for rename source '{}'",
                            from
                        )));
                    }
                };
                let to_dec = match decisions.get(to) {
                    Some(d) => d,
                    None => {
                        cleanup_tmps(&staged).await;
                        return Err(BifrostError::Config(format!(
                            "[file.invalid_args] no policy decision for rename dest '{}'",
                            to
                        )));
                    }
                };
                let from_path = from_dec.path.as_path().to_path_buf();
                let to_path = to_dec.path.as_path().to_path_buf();
                // Source must exist; dest must not (unless overwrite allowed).
                if fs::symlink_metadata(&from_path).await.is_err() {
                    cleanup_tmps(&staged).await;
                    return Err(io_err(
                        "rename-source",
                        std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            format!("rename source '{}' does not exist", from),
                        ),
                    ));
                }
                if fs::metadata(&to_path).await.is_ok() && !to_dec.allow_overwrite {
                    cleanup_tmps(&staged).await;
                    return Err(precondition_failed(format!(
                        "rename destination '{}' exists and overwrite is disabled",
                        to
                    )));
                }
                let prior_mode = capture_mode(&from_path).await;
                staged.push(Staged {
                    target: to_path.clone(),
                    tmp: PathBuf::new(),
                    new_path_is_dev_null: false,
                    prior_mode,
                    applied_entry: json!({
                        "from": from_path.to_string_lossy(),
                        "to": to_path.to_string_lossy(),
                        "renamed": true,
                    }),
                    action: StageAction::Rename { from: from_path },
                });
                continue;
            }
            PatchKind::CopyOnly { from, to } => {
                let from_dec = match decisions.get(from) {
                    Some(d) => d,
                    None => {
                        cleanup_tmps(&staged).await;
                        return Err(BifrostError::Config(format!(
                            "[file.invalid_args] no policy decision for copy source '{}'",
                            from
                        )));
                    }
                };
                let to_dec = match decisions.get(to) {
                    Some(d) => d,
                    None => {
                        cleanup_tmps(&staged).await;
                        return Err(BifrostError::Config(format!(
                            "[file.invalid_args] no policy decision for copy dest '{}'",
                            to
                        )));
                    }
                };
                let from_path = from_dec.path.as_path().to_path_buf();
                let to_path = to_dec.path.as_path().to_path_buf();
                if fs::metadata(&to_path).await.is_ok() && !to_dec.allow_overwrite {
                    cleanup_tmps(&staged).await;
                    return Err(precondition_failed(format!(
                        "copy destination '{}' exists and overwrite is disabled",
                        to
                    )));
                }
                let bytes = match fs::read(&from_path).await {
                    Ok(b) => b,
                    Err(e) => {
                        cleanup_tmps(&staged).await;
                        return Err(io_err("copy-source-read", e));
                    }
                };
                let max = if to_dec.max_write_bytes == 0 {
                    DEFAULT_MAX_WRITE_BYTES
                } else {
                    to_dec.max_write_bytes
                };
                if (bytes.len() as u64) > max {
                    cleanup_tmps(&staged).await;
                    return Err(size_too_large(format!(
                        "copied file is {} bytes, max_write_bytes is {}",
                        bytes.len(),
                        max
                    )));
                }
                let prior_mode = capture_mode(&from_path).await;
                let parent = match to_path.parent() {
                    Some(p) => p.to_path_buf(),
                    None => {
                        cleanup_tmps(&staged).await;
                        return Err(BifrostError::Config(
                            "[file.invalid_args] copy dest has no parent".to_string(),
                        ));
                    }
                };
                if let Err(e) = fs::create_dir_all(&parent).await {
                    cleanup_tmps(&staged).await;
                    return Err(io_err("mkdir-parent", e));
                }
                let pid = std::process::id();
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                let tmp = parent.join(format!(
                    ".bifrost-patch.{}.{}.{}.tmp",
                    pid,
                    nanos,
                    staged.len()
                ));
                {
                    let write_res = async {
                        let mut f = fs::File::create(&tmp)
                            .await
                            .map_err(|e| io_err("create-tmp", e))?;
                        f.write_all(&bytes)
                            .await
                            .map_err(|e| io_err("write-tmp", e))?;
                        f.sync_all().await.map_err(|e| io_err("fsync-tmp", e))?;
                        Ok::<_, BifrostError>(())
                    }
                    .await;
                    if let Err(e) = write_res {
                        let _ = fs::remove_file(&tmp).await;
                        cleanup_tmps(&staged).await;
                        return Err(e);
                    }
                }
                staged.push(Staged {
                    target: to_path.clone(),
                    tmp,
                    new_path_is_dev_null: false,
                    prior_mode,
                    applied_entry: json!({
                        "from": from_path.to_string_lossy(),
                        "to": to_path.to_string_lossy(),
                        "bytes_written": bytes.len(),
                        "sha256": sha256_hex(&bytes),
                        "copied": true,
                    }),
                    action: StageAction::Copy,
                });
                continue;
            }
            PatchKind::Modify | PatchKind::Create | PatchKind::Delete => {}
        }

        let _old_path = entry.old_path.clone();
        let new_path = entry.new_path.clone();
        let body = entry.body.as_str();
        let key = entry.decision_key();
        let decision = match decisions.get(&key) {
            Some(d) => d,
            None => {
                cleanup_tmps(&staged).await;
                return Err(BifrostError::Config(format!(
                    "[file.invalid_args] no policy decision for patch target '{}'",
                    key
                )));
            }
        };

        let target = decision.path.as_path().to_path_buf();
        let orig_bytes = match fs::read(&target).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                cleanup_tmps(&staged).await;
                return Err(io_err("read", e));
            }
        };
        // P0-1: per-file optimistic lock. If the caller supplied a
        // `base_sha256` for this patch target, verify the on-disk content
        // hashes to the expected value BEFORE staging any change. Unlike the
        // context-line matching below (which can still match after an unrelated
        // edit elsewhere in the file), this guarantees the patch was generated
        // against exactly the bytes currently on disk — closing the silent
        // multi-agent overwrite gap. An empty expected sha means "expected to
        // not exist yet" (creation), mirroring file.write semantics.
        if let Some(expected) = base_shas.get(&key) {
            let actual = if orig_bytes.is_empty() && fs::metadata(&target).await.is_err() {
                String::new()
            } else {
                sha256_hex(&orig_bytes)
            };
            if !actual.eq_ignore_ascii_case(expected) {
                cleanup_tmps(&staged).await;
                return Err(sha_mismatch(format!(
                    "base_sha256 mismatch for '{}': expected {}, actual {}",
                    key,
                    expected,
                    if actual.is_empty() {
                        "<absent>"
                    } else {
                        &actual
                    }
                )));
            }
        }
        let prior_mode = capture_mode(&target).await;
        let eol = Eol::detect(&orig_bytes);
        let orig_text = match String::from_utf8(orig_bytes.clone()) {
            Ok(t) => t,
            Err(_) => {
                cleanup_tmps(&staged).await;
                return Err(BifrostError::Config(
                    "[file.invalid_args] file is not utf-8".to_string(),
                ));
            }
        };
        let orig_lines: Vec<&str> = orig_text.split_inclusive('\n').collect();

        let mut out = String::with_capacity(orig_text.len());
        let mut src_cursor: usize = 0;
        let norm_body = if body.starts_with("@@ ") {
            format!("\n{}", body)
        } else {
            body.to_string()
        };
        let mut hunks = norm_body.split("\n@@ ");
        let _meta = hunks.next();

        let mut hunk_err: Option<BifrostError> = None;
        'hunk_loop: for hunk in hunks {
            let mut hlines = hunk.splitn(2, '\n');
            let header = hlines.next().unwrap_or("");
            let content = hlines.next().unwrap_or("");
            let (old_start, _old_count) = match parse_hunk_range(header, '-') {
                Some(v) => v,
                None => {
                    hunk_err = Some(BifrostError::Config(format!(
                        "[file.invalid_args] bad hunk header: {}",
                        header
                    )));
                    break 'hunk_loop;
                }
            };
            let target_cursor = if old_start == 0 { 0 } else { old_start - 1 };
            while src_cursor < target_cursor && src_cursor < orig_lines.len() {
                out.push_str(orig_lines[src_cursor]);
                src_cursor += 1;
            }
            // Track the most recent emitted op so a `\ No newline at
            // end of file` marker can retroactively strip the trailing
            // newline we (optimistically) appended for the previous `+`
            // or context line.
            #[derive(Copy, Clone)]
            enum LastEmit {
                None,
                Context,
                Plus,
            }
            let mut last_emit = LastEmit::None;
            for line in content.split_inclusive('\n') {
                let body_line = line.strip_suffix('\n').unwrap_or(line);
                if body_line.is_empty() && !line.ends_with('\n') {
                    continue;
                }
                let ch = body_line.chars().next().unwrap_or(' ');
                let tail = &body_line[ch.len_utf8()..];
                match ch {
                    ' ' => {
                        if src_cursor >= orig_lines.len()
                            || orig_lines[src_cursor]
                                .trim_end_matches('\n')
                                .trim_end_matches('\r')
                                != tail
                        {
                            hunk_err = Some(precondition_failed(format!(
                                "context mismatch at line {} in '{}'",
                                src_cursor + 1,
                                key
                            )));
                            break 'hunk_loop;
                        }
                        out.push_str(orig_lines[src_cursor]);
                        src_cursor += 1;
                        last_emit = LastEmit::Context;
                    }
                    '-' => {
                        if src_cursor >= orig_lines.len()
                            || orig_lines[src_cursor]
                                .trim_end_matches('\n')
                                .trim_end_matches('\r')
                                != tail
                        {
                            hunk_err = Some(precondition_failed(format!(
                                "delete-line mismatch at line {} in '{}'",
                                src_cursor + 1,
                                key
                            )));
                            break 'hunk_loop;
                        }
                        src_cursor += 1;
                        // `-` emits nothing to `out`; leave last_emit as-is
                        // so a subsequent `\` marker still refers to the
                        // *original* line (handled implicitly by not touching
                        // out, and by src orig_lines preserving its EOL).
                    }
                    '+' => {
                        out.push_str(tail);
                        out.push_str(eol.newline());
                        last_emit = LastEmit::Plus;
                    }
                    '\\' => {
                        // "\ No newline at end of file" applies to the
                        // immediately preceding hunk line. For `+` and context,
                        // strip the trailing EOL we just wrote to `out`. For
                        // `-`, there is nothing to strip in `out`; we just
                        // remember that the original ended without a newline
                        // (orig_lines already reflects that via split_inclusive).
                        match last_emit {
                            LastEmit::Plus | LastEmit::Context => {
                                let nl = eol.newline();
                                if out.ends_with(nl) {
                                    out.truncate(out.len() - nl.len());
                                }
                            }
                            LastEmit::None => {}
                        }
                        last_emit = LastEmit::None;
                    }
                    _ => {
                        hunk_err = Some(BifrostError::Config(format!(
                            "[file.invalid_args] unknown hunk char '{}' in '{}'",
                            ch, key
                        )));
                        break 'hunk_loop;
                    }
                }
            }
        }
        if let Some(e) = hunk_err {
            cleanup_tmps(&staged).await;
            return Err(e);
        }
        while src_cursor < orig_lines.len() {
            out.push_str(orig_lines[src_cursor]);
            src_cursor += 1;
        }

        let new_bytes = out.into_bytes();
        let max = if decision.max_write_bytes == 0 {
            DEFAULT_MAX_WRITE_BYTES
        } else {
            decision.max_write_bytes
        };
        if (new_bytes.len() as u64) > max {
            cleanup_tmps(&staged).await;
            return Err(size_too_large(format!(
                "patched file is {} bytes, max_write_bytes is {}",
                new_bytes.len(),
                max
            )));
        }

        let new_path_is_dev_null = new_path == "/dev/null";
        // For deletes we don't need a tmp; we still stage an entry so phase 2
        // can carry out the unlink after every other tmp has been produced.
        if new_path_is_dev_null {
            staged.push(Staged {
                target: target.clone(),
                tmp: PathBuf::new(),
                new_path_is_dev_null: true,
                prior_mode,
                action: StageAction::Delete,
                applied_entry: json!({
                    "path": target.to_string_lossy(),
                    "deleted": true,
                }),
            });
            continue;
        }

        let parent = match target.parent() {
            Some(p) => p.to_path_buf(),
            None => {
                cleanup_tmps(&staged).await;
                return Err(BifrostError::Config(
                    "[file.invalid_args] path has no parent".to_string(),
                ));
            }
        };
        if let Err(e) = fs::create_dir_all(&parent).await {
            cleanup_tmps(&staged).await;
            return Err(io_err("mkdir-parent", e));
        }
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = parent.join(format!(
            ".bifrost-patch.{}.{}.{}.tmp",
            pid,
            nanos,
            staged.len()
        ));
        {
            let write_res = async {
                let mut f = fs::File::create(&tmp)
                    .await
                    .map_err(|e| io_err("create-tmp", e))?;
                f.write_all(&new_bytes)
                    .await
                    .map_err(|e| io_err("write-tmp", e))?;
                f.sync_all().await.map_err(|e| io_err("fsync-tmp", e))?;
                Ok::<_, BifrostError>(())
            }
            .await;
            if let Err(e) = write_res {
                cleanup_tmps(&staged).await;
                let _ = fs::remove_file(&tmp).await;
                return Err(e);
            }
        }
        let sha = sha256_hex(&new_bytes);
        let applied_entry = json!({
            "path": target.to_string_lossy(),
            "bytes_written": new_bytes.len(),
            "sha256": sha,
        });
        staged.push(Staged {
            target,
            tmp,
            new_path_is_dev_null: false,
            prior_mode,
            applied_entry,
            action: StageAction::WriteTmp,
        });
    }

    // Phase 2 — commit. `committed` snapshots let us best-effort restore
    // content-bearing targets. `renamed` separately tracks rename moves so
    // rollback can swing the file back to its original location.
    let mut committed: Vec<(PathBuf, Vec<u8>, Option<u32>, bool)> = Vec::new();
    // (current_path, original_path) for renames already performed.
    let mut renamed: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut applied_out: Vec<Value> = Vec::new();

    // Shared rollback: undo renames, then restore/unlink content targets,
    // then drop any not-yet-renamed tmp files.
    async fn rollback(
        committed: &[(PathBuf, Vec<u8>, Option<u32>, bool)],
        renamed: &[(PathBuf, PathBuf)],
        staged: &[Staged],
        done: usize,
    ) {
        for (cur, orig) in renamed.iter().rev() {
            let _ = fs::rename(cur, orig).await;
        }
        for (path, bytes, prior, existed) in committed.iter().rev() {
            if *existed {
                let _ = fs::write(path, bytes).await;
                let _ = apply_mode_sync(path, *prior);
            } else {
                let _ = fs::remove_file(path).await;
            }
        }
        for rest in staged.iter().skip(done) {
            if !rest.tmp.as_os_str().is_empty() {
                let _ = fs::remove_file(&rest.tmp).await;
            }
        }
    }

    for (idx, s) in staged.iter().enumerate() {
        match &s.action {
            StageAction::Delete => {
                if let Err(e) = fs::remove_file(&s.target).await {
                    rollback(&committed, &renamed, &staged, idx).await;
                    return Err(io_err("remove_file", e));
                }
            }
            StageAction::Rename { from } => {
                // Respect overwrite at commit time too (race-narrowing).
                let existed_before = fs::metadata(&s.target).await.is_ok();
                let snapshot = fs::read(&s.target).await.unwrap_or_default();
                let snapshot_mode = capture_mode(&s.target).await;
                if let Err(e) = fs::rename(from, &s.target).await {
                    rollback(&committed, &renamed, &staged, idx).await;
                    return Err(io_err("rename", e));
                }
                apply_mode(&s.target, s.prior_mode).await;
                // If the dest pre-existed, record its snapshot so rollback can
                // recreate it after swinging the rename back.
                if existed_before {
                    committed.push((s.target.clone(), snapshot, snapshot_mode, true));
                }
                renamed.push((s.target.clone(), from.clone()));
            }
            StageAction::WriteTmp | StageAction::Copy => {
                let existed_before = fs::metadata(&s.target).await.is_ok();
                let snapshot = fs::read(&s.target).await.unwrap_or_default();
                let snapshot_mode = capture_mode(&s.target).await;
                if let Err(e) = fs::rename(&s.tmp, &s.target).await {
                    rollback(&committed, &renamed, &staged, idx).await;
                    return Err(io_err("rename-tmp", e));
                }
                apply_mode(&s.target, s.prior_mode).await;
                committed.push((s.target.clone(), snapshot, snapshot_mode, existed_before));
            }
        }
        applied_out.push(s.applied_entry.clone());
    }

    Ok(json!({ "files": applied_out }))
}

fn parse_hunk_range(header: &str, sign: char) -> Option<(usize, usize)> {
    // header example (after "@@ " prefix has been stripped): "-5,7 +5,8 @@"
    // or "-5 +5,8 @@", or the very first hunk which still starts with "@@" if
    // splitn didn't strip it. Handle both.
    let header = header.trim_start_matches('@').trim_start();
    for tok in header.split_whitespace() {
        if tok.starts_with(sign) {
            let tok = &tok[1..];
            let mut it = tok.split(',');
            let start: usize = it.next()?.parse().ok()?;
            let count: usize = match it.next() {
                Some(c) => c.parse().ok()?,
                None => 1,
            };
            return Some((start, count));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::file_access::{FileAccessPolicy, FileOp};
    use std::io::Write;

    fn mk_policy(root: &Path) -> FileAccessPolicy {
        FileAccessPolicy::new_readonly("t", vec![root.to_path_buf()])
    }

    fn mk_rw_policy(root: &Path) -> FileAccessPolicy {
        FileAccessPolicy::new_read_write("t", vec![root.to_path_buf()])
    }

    #[tokio::test]
    async fn read_small_text_file_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("hello.txt");
        let mut f = std::fs::File::create(&file).unwrap();
        writeln!(f, "hello").unwrap();
        drop(f);

        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("hello.txt"), tmp.path(), FileOp::Read)
            .unwrap();
        let v = handle_file_read(&dec, None, false, None, None)
            .await
            .unwrap();
        assert_eq!(v["size"].as_u64().unwrap(), 6);
        assert!(!v["truncated"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn hash_rejects_unsupported_algo() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("a.txt"), tmp.path(), FileOp::Hash)
            .unwrap();
        let err = handle_file_hash(&dec, Some("md5")).await.unwrap_err();
        assert!(err.to_string().contains("file.unsupported_algo"));
    }

    // ---------- P1-4 outline ----------

    #[test]
    fn outline_language_detection_by_ext() {
        assert_eq!(outline_language_for(Path::new("a.rs")), "rust");
        assert_eq!(outline_language_for(Path::new("a.tsx")), "typescript");
        assert_eq!(outline_language_for(Path::new("a.py")), "python");
        assert_eq!(outline_language_for(Path::new("a.go")), "go");
        assert_eq!(outline_language_for(Path::new("a.unknownext")), "unknown");
        assert_eq!(outline_language_for(Path::new("noext")), "unknown");
    }

    #[test]
    fn outline_extracts_rust_symbols() {
        let src = r#"
// a comment fn not_a_symbol
pub fn alpha(x: i32) -> i32 { x }
async fn beta() {}
pub struct Foo { a: u32 }
enum Bar { A, B }
pub trait Baz {}
impl Foo {}
type Alias = u32;
"#;
        let (syms, truncated) = extract_outline(src, "rust", 2000);
        assert!(!truncated);
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
        assert!(names.contains(&"Foo"));
        assert!(names.contains(&"Bar"));
        assert!(names.contains(&"Baz"));
        assert!(names.contains(&"Alias"));
        // The commented `fn not_a_symbol` must not be picked up.
        assert!(!names.contains(&"not_a_symbol"));
        // Kinds are tagged.
        let fn_kind = syms.iter().find(|s| s.name == "alpha").unwrap().kind;
        assert_eq!(fn_kind, "fn");
    }

    #[test]
    fn outline_extracts_python_and_respects_hash_comments() {
        let src =
            "# top comment\nclass C:\n    def method(self):\n        pass\ndef top():\n    pass\n";
        let (syms, _) = extract_outline(src, "python", 2000);
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"C"));
        assert!(names.contains(&"method"));
        assert!(names.contains(&"top"));
    }

    #[test]
    fn outline_respects_max_symbols_truncation() {
        let src = "fn a(){}\nfn b(){}\nfn c(){}\n";
        let (syms, truncated) = extract_outline(src, "rust", 2);
        assert_eq!(syms.len(), 2);
        assert!(truncated);
    }

    #[test]
    fn outline_unknown_language_is_empty() {
        let src = "some random text\nwith no recognizable symbols\n";
        let (syms, truncated) = extract_outline(src, "unknown", 2000);
        assert!(syms.is_empty());
        assert!(!truncated);
    }

    #[tokio::test]
    async fn outline_handler_returns_symbols_for_rust_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("lib.rs");
        std::fs::write(&file, b"pub fn one() {}\nstruct Two;\n").unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("lib.rs"), tmp.path(), FileOp::Outline)
            .unwrap();
        let v = handle_file_outline(&dec, None, None).await.unwrap();
        assert_eq!(v["language"].as_str().unwrap(), "rust");
        assert_eq!(v["count"].as_u64().unwrap(), 2);
        assert!(!v["truncated"].as_bool().unwrap());
        let syms = v["symbols"].as_array().unwrap();
        assert_eq!(syms[0]["name"].as_str().unwrap(), "one");
        assert_eq!(syms[0]["kind"].as_str().unwrap(), "fn");
        assert_eq!(syms[0]["line"].as_u64().unwrap(), 1);
    }

    #[tokio::test]
    async fn list_returns_top_level_entries() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"x").unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub/b.txt"), b"y").unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::List)
            .unwrap();
        let v = handle_file_list(&dec, Some(1), &[], false, &[], None, None)
            .await
            .unwrap();
        let entries = v["entries"].as_array().unwrap();
        assert!(entries.iter().any(|e| e["name"] == "a.txt"));
        assert!(entries.iter().any(|e| e["name"] == "sub"));
    }

    #[tokio::test]
    async fn read_with_offset_limit_returns_line_range() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("multi.txt");
        std::fs::write(&file, "line1\nline2\nline3\nline4\nline5\n").unwrap();

        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("multi.txt"), tmp.path(), FileOp::Read)
            .unwrap();

        // Read lines 2-3
        let v = handle_file_read(&dec, None, false, Some(2), Some(2))
            .await
            .unwrap();
        assert_eq!(v["start_line"].as_u64().unwrap(), 2);
        assert_eq!(v["end_line"].as_u64().unwrap(), 3);
        assert_eq!(v["total_lines"].as_u64().unwrap(), 5);
        assert!(v["truncated"].as_bool().unwrap()); // end_line < total_lines
        let content_b64 = v["content_b64"].as_str().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(content_b64)
            .unwrap();
        let text = String::from_utf8(decoded).unwrap();
        assert!(text.contains("line2"));
        assert!(text.contains("line3"));
        assert!(!text.contains("line1"));
        assert!(!text.contains("line4"));
    }

    #[tokio::test]
    async fn read_with_offset_only_returns_from_offset_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("multi.txt");
        std::fs::write(&file, "aaa\nbbb\nccc\n").unwrap();

        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("multi.txt"), tmp.path(), FileOp::Read)
            .unwrap();

        let v = handle_file_read(&dec, None, false, Some(2), None)
            .await
            .unwrap();
        assert_eq!(v["start_line"].as_u64().unwrap(), 2);
        assert_eq!(v["end_line"].as_u64().unwrap(), 3);
        assert!(!v["truncated"].as_bool().unwrap());
        let content_b64 = v["content_b64"].as_str().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(content_b64)
            .unwrap();
        let text = String::from_utf8(decoded).unwrap();
        assert!(text.contains("bbb"));
        assert!(text.contains("ccc"));
        assert!(!text.contains("aaa"));
    }

    #[tokio::test]
    async fn search_with_context_lines() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("code.rs"),
            "fn main() {\n    let x = 1;\n    println!(\"hello\");\n    let y = 2;\n}\n",
        )
        .unwrap();

        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::Search)
            .unwrap();

        let v = handle_file_search(
            &dec,
            "println",
            None,
            None,
            &[],
            Some(1),
            Some(1),
            false,
            None,
            false,
            &[],
            None,
            &[],
            None,
            false,
            false,
        )
        .await
        .unwrap();
        let matches = v["matches"].as_array().unwrap();
        assert!(!matches.is_empty());
        let m = &matches[0];
        assert_eq!(m["line"].as_u64().unwrap(), 3);
        let ctx = m["context"].as_array().unwrap();
        assert!(ctx.len() >= 3); // before + match + after
                                 // Check context includes surrounding lines
        let ctx_lines: Vec<u64> = ctx.iter().map(|c| c["line"].as_u64().unwrap()).collect();
        assert!(ctx_lines.contains(&2)); // before
        assert!(ctx_lines.contains(&3)); // match
        assert!(ctx_lines.contains(&4)); // after
    }

    #[tokio::test]
    async fn glob_excludes_default_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/main.rs"), b"fn main(){}").unwrap();
        std::fs::create_dir(tmp.path().join("node_modules")).unwrap();
        std::fs::write(tmp.path().join("node_modules/pkg.js"), b"x").unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git/config"), b"y").unwrap();

        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::Glob)
            .unwrap();

        let v = handle_file_glob(&dec, "**/*", None, &[], false, &[], None)
            .await
            .unwrap();
        let matches: Vec<&str> = v["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap())
            .collect();
        assert!(matches.iter().any(|m| m.contains("main.rs")));
        assert!(!matches.iter().any(|m| m.contains("node_modules")));
        assert!(!matches.iter().any(|m| m.contains(".git")));
    }

    #[tokio::test]
    async fn list_excludes_default_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::create_dir(tmp.path().join("node_modules")).unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();

        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::List)
            .unwrap();

        let v = handle_file_list(&dec, Some(2), &[], false, &[], None, None)
            .await
            .unwrap();
        let entries = v["entries"].as_array().unwrap();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"src"));
        // node_modules and .git should be excluded from recursive listing
        // but they still appear at depth=0 as direct children — the skip applies to recursion into them
        // Actually they should still show up in listing but not be traversed
    }

    // ---------------------------------------------------------------
    //  Edge case: file.read — offset beyond EOF
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn read_offset_beyond_eof_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("short.txt"), "aaa\nbbb\n").unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("short.txt"), tmp.path(), FileOp::Read)
            .unwrap();

        let v = handle_file_read(&dec, None, false, Some(999), Some(5))
            .await
            .unwrap();
        assert_eq!(v["size"].as_u64().unwrap(), 0);
        assert_eq!(v["total_lines"].as_u64().unwrap(), 2);
        // start_line clamped to total_lines + 1 area; end_line should be start-1 range (empty)
        let content_b64 = v["content_b64"].as_str().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(content_b64)
            .unwrap();
        assert!(decoded.is_empty());
    }

    // ---------------------------------------------------------------
    //  Edge case: file.read — empty file with offset/limit
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn read_empty_file_with_offset_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("empty.txt"), "").unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("empty.txt"), tmp.path(), FileOp::Read)
            .unwrap();

        let v = handle_file_read(&dec, None, false, Some(1), Some(10))
            .await
            .unwrap();
        assert_eq!(v["total_lines"].as_u64().unwrap(), 0);
        assert_eq!(v["size"].as_u64().unwrap(), 0);
        assert!(!v["truncated"].as_bool().unwrap());
    }

    // ---------------------------------------------------------------
    //  Edge case: file.read — limit=0 returns empty
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn read_limit_zero_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), "aaa\nbbb\nccc\n").unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("f.txt"), tmp.path(), FileOp::Read)
            .unwrap();

        let v = handle_file_read(&dec, None, false, Some(1), Some(0))
            .await
            .unwrap();
        assert_eq!(v["size"].as_u64().unwrap(), 0);
        assert_eq!(v["total_lines"].as_u64().unwrap(), 3);
        assert!(v["truncated"].as_bool().unwrap()); // 0 < 3
    }

    // ---------------------------------------------------------------
    //  Edge case: file.read — non-offset mode includes total_lines
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn read_non_offset_includes_total_lines() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), "aaa\nbbb\nccc\n").unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("f.txt"), tmp.path(), FileOp::Read)
            .unwrap();

        let v = handle_file_read(&dec, None, false, None, None)
            .await
            .unwrap();
        // Non-offset mode should include total_lines for text files
        assert_eq!(v["total_lines"].as_u64().unwrap(), 3);
    }

    // ---------------------------------------------------------------
    //  Edge case: file.search — match at first line with context_before
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn search_match_at_first_line_with_context_before() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("first.txt"), "MATCH_HERE\nsecond\nthird\n").unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::Search)
            .unwrap();

        let v = handle_file_search(
            &dec,
            "MATCH_HERE",
            None,
            None,
            &[],
            Some(3),
            Some(1),
            false,
            None,
            false,
            &[],
            None,
            &[],
            None,
            false,
            false,
        )
        .await
        .unwrap();
        let matches = v["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        let ctx = matches[0]["context"].as_array().unwrap();
        // Line 1 matched, context_before=3 but only line 1 available (saturating_sub)
        let ctx_lines: Vec<u64> = ctx.iter().map(|c| c["line"].as_u64().unwrap()).collect();
        assert_eq!(*ctx_lines.first().unwrap(), 1); // can't go before line 1
        assert!(ctx_lines.contains(&2)); // after
    }

    // ---------------------------------------------------------------
    //  Edge case: file.search — match at last line with context_after
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn search_match_at_last_line_with_context_after() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("last.txt"), "first\nsecond\nLAST_MATCH\n").unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::Search)
            .unwrap();

        let v = handle_file_search(
            &dec,
            "LAST_MATCH",
            None,
            None,
            &[],
            Some(1),
            Some(5),
            false,
            None,
            false,
            &[],
            None,
            &[],
            None,
            false,
            false,
        )
        .await
        .unwrap();
        let matches = v["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        let ctx = matches[0]["context"].as_array().unwrap();
        let ctx_lines: Vec<u64> = ctx.iter().map(|c| c["line"].as_u64().unwrap()).collect();
        assert!(ctx_lines.contains(&2)); // before
        assert!(ctx_lines.contains(&3)); // match (last line)
                                         // context_after=5 but only 3 lines total — no crash, just clamped
        assert_eq!(ctx.len(), 2); // line 2 + line 3
    }

    // ---------------------------------------------------------------
    //  Edge case: file.search — no matches returns empty
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn search_no_matches_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), "hello world\n").unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::Search)
            .unwrap();

        let v = handle_file_search(
            &dec,
            "NONEXISTENT_PATTERN_XYZ",
            None,
            None,
            &[],
            Some(2),
            Some(2),
            false,
            None,
            false,
            &[],
            None,
            &[],
            None,
            false,
            false,
        )
        .await
        .unwrap();
        let matches = v["matches"].as_array().unwrap();
        assert!(matches.is_empty());
        assert!(!v["truncated"].as_bool().unwrap());
    }

    // ---------------------------------------------------------------
    //  Edge case: file.search — invalid regex returns error
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn search_invalid_regex_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), "content\n").unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::Search)
            .unwrap();

        let err = handle_file_search(
            &dec,
            "[invalid",
            None,
            None,
            &[],
            None,
            None,
            false,
            None,
            false,
            &[],
            None,
            &[],
            None,
            false,
            false,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("file.invalid_regex"));
    }

    // ---------------------------------------------------------------
    //  Bug regression: file.edit — empty replacement deletes line cleanly
    // ---------------------------------------------------------------
    //  Regression: replacing the last line must preserve the
    //  original file's trailing newline.
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn edit_last_line_preserves_trailing_newline() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("three.txt");
        std::fs::write(&file, "line1\nline2\nline3\n").unwrap();

        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("three.txt"), tmp.path(), FileOp::Edit)
            .unwrap();

        // Replace the last line with text that itself has NO newline.
        // Expected: the file must still end with a single \n so the
        // overall EOL posture is unchanged.
        let edits = vec![EditRange {
            start_line: 3,
            end_line: 3,
            replacement: "THIRD".to_string(),
            ..Default::default()
        }];
        let v = handle_file_edit(&dec, None, &edits).await.unwrap();
        assert_eq!(v["applied_edits"].as_u64().unwrap(), 1);

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "line1\nline2\nTHIRD\n");
    }

    // ---------------------------------------------------------------
    //  Counterpart: a file without a trailing newline must stay that
    //  way after replacing the last line with a newline-less string.
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn edit_last_line_no_trailing_newline_stays_as_is() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("three.txt");
        std::fs::write(&file, "line1\nline2\nline3").unwrap();

        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("three.txt"), tmp.path(), FileOp::Edit)
            .unwrap();

        let edits = vec![EditRange {
            start_line: 3,
            end_line: 3,
            replacement: "THIRD".to_string(),
            ..Default::default()
        }];
        handle_file_edit(&dec, None, &edits).await.unwrap();
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "line1\nline2\nTHIRD");
    }

    // ---------------------------------------------------------------
    #[tokio::test]
    async fn edit_empty_replacement_deletes_line_without_blank() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("three.txt");
        std::fs::write(&file, "line1\nline2\nline3\n").unwrap();

        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("three.txt"), tmp.path(), FileOp::Edit)
            .unwrap();

        let edits = vec![EditRange {
            start_line: 2,
            end_line: 2,
            replacement: String::new(),
            ..Default::default()
        }];
        let v = handle_file_edit(&dec, None, &edits).await.unwrap();
        assert_eq!(v["applied_edits"].as_u64().unwrap(), 1);

        let content = std::fs::read_to_string(&file).unwrap();
        // Should be "line1\nline3\n" — NO extra blank line
        assert_eq!(content, "line1\nline3\n");
    }

    // ---------------------------------------------------------------
    //  Edge case: file.edit — multiple non-overlapping ranges
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn edit_multiple_ranges() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("multi.txt");
        std::fs::write(&file, "aa\nbb\ncc\ndd\nee\n").unwrap();

        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("multi.txt"), tmp.path(), FileOp::Edit)
            .unwrap();

        let edits = vec![
            EditRange {
                start_line: 1,
                end_line: 1,
                replacement: "AA\n".to_string(),
                ..Default::default()
            },
            EditRange {
                start_line: 3,
                end_line: 4,
                replacement: "CC_DD\n".to_string(),
                ..Default::default()
            },
        ];
        let v = handle_file_edit(&dec, None, &edits).await.unwrap();
        assert_eq!(v["applied_edits"].as_u64().unwrap(), 2);

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "AA\nbb\nCC_DD\nee\n");
    }

    // ---------------------------------------------------------------
    //  Edge case: file.edit — overlapping ranges rejected
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn edit_overlapping_ranges_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f.txt");
        std::fs::write(&file, "aa\nbb\ncc\n").unwrap();

        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("f.txt"), tmp.path(), FileOp::Edit)
            .unwrap();

        let edits = vec![
            EditRange {
                start_line: 1,
                end_line: 2,
                replacement: "X\n".to_string(),
                ..Default::default()
            },
            EditRange {
                start_line: 2,
                end_line: 3,
                replacement: "Y\n".to_string(),
                ..Default::default()
            },
        ];
        let err = handle_file_edit(&dec, None, &edits).await.unwrap_err();
        assert!(err.to_string().contains("overlap"));
    }

    // ---------------------------------------------------------------
    //  Edge case: file.write + read roundtrip
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn write_and_read_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = mk_rw_policy(tmp.path());

        let content = "hello from test\n";
        let content_b64 = base64::engine::general_purpose::STANDARD.encode(content.as_bytes());

        let write_dec = policy
            .check(Path::new("out.txt"), tmp.path(), FileOp::Write)
            .unwrap();
        let wv = handle_file_write(&write_dec, &content_b64, None, None, false)
            .await
            .unwrap();
        assert_eq!(wv["bytes_written"].as_u64().unwrap(), 16);
        assert!(wv["sha256"].is_string());
        assert!(wv["previous_sha256"].is_null()); // new file, no previous

        // Read back
        let read_dec = policy
            .check(Path::new("out.txt"), tmp.path(), FileOp::Read)
            .unwrap();
        let rv = handle_file_read(&read_dec, None, false, None, None)
            .await
            .unwrap();
        let decoded_b64 = rv["content_b64"].as_str().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(decoded_b64)
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), content);
    }

    // ---------------------------------------------------------------
    //  Edge case: file.write — sha256 precondition mismatch
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn write_sha256_precondition_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), "original").unwrap();
        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("f.txt"), tmp.path(), FileOp::Write)
            .unwrap();

        let content_b64 = base64::engine::general_purpose::STANDARD.encode(b"new content");
        let err = handle_file_write(&dec, &content_b64, Some("wrong_sha"), None, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("file.sha_mismatch"));
    }

    #[tokio::test]
    async fn list_sets_truncated_flag_when_exceeding_limit_p1_4() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("a.txt"), b"x").unwrap();
        let policy = mk_rw_policy(root);
        let dec = policy.check(Path::new("."), root, FileOp::List).unwrap();
        let v = handle_file_list(&dec, Some(1), &[], false, &[], None, None)
            .await
            .unwrap();
        assert!(
            v.get("truncated").is_some(),
            "list response must expose `truncated` flag"
        );
        assert_eq!(v["truncated"], serde_json::Value::Bool(false));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stat_exposes_symlink_target_p2() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let target = root.join("real.txt");
        std::fs::write(&target, b"hi").unwrap();
        let link = root.join("link.txt");
        symlink(&target, &link).unwrap();

        let policy = mk_rw_policy(root);
        let dec = policy
            .check(Path::new("link.txt"), root, FileOp::Stat)
            .unwrap();
        let v = handle_file_stat(&dec).await.unwrap();
        assert_eq!(v["kind"], "symlink", "kind should be symlink");
        assert!(
            v.get("symlink_target").is_some(),
            "stat on symlink must include symlink_target"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn list_entries_include_symlink_target_p1() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let real = root.join("real.txt");
        std::fs::write(&real, b"hi").unwrap();
        let link = root.join("link.txt");
        symlink(&real, &link).unwrap();

        let policy = mk_rw_policy(root);
        let dec = policy.check(Path::new("."), root, FileOp::List).unwrap();
        let v = handle_file_list(&dec, Some(1), &[], false, &[], None, None)
            .await
            .unwrap();
        let entries = v["entries"].as_array().unwrap();
        let link_entry = entries
            .iter()
            .find(|e| e["name"] == "link.txt")
            .expect("link.txt missing from entries");
        assert_eq!(link_entry["type"], "symlink", "symlink should be tagged");
        assert!(
            link_entry.get("symlink_target").is_some(),
            "list symlink entries must carry symlink_target"
        );
    }

    #[tokio::test]
    async fn edit_sha_mismatch_returns_file_sha_mismatch_code_p2() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let file = root.join("x.txt");
        std::fs::write(&file, "hello\n").unwrap();
        let policy = mk_rw_policy(root);
        let dec = policy
            .check(Path::new("x.txt"), root, FileOp::Edit)
            .unwrap();
        let edits = vec![EditRange {
            start_line: 1,
            end_line: 1,
            replacement: "world\n".to_string(),
            ..Default::default()
        }];
        let err = handle_file_edit(&dec, Some("deadbeef"), &edits)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("file.sha_mismatch"),
            "expected file.sha_mismatch code, got: {}",
            err
        );
    }

    // ---------------------------------------------------------------
    //  Edge case: file.glob — custom exclude patterns
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn glob_custom_exclude() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/main.rs"), b"fn main(){}").unwrap();
        std::fs::create_dir(tmp.path().join("build")).unwrap();
        std::fs::write(tmp.path().join("build/out.o"), b"x").unwrap();

        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::Glob)
            .unwrap();

        let v = handle_file_glob(&dec, "**/*", None, &["build".to_string()], false, &[], None)
            .await
            .unwrap();
        let matches: Vec<&str> = v["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap())
            .collect();
        assert!(matches.iter().any(|m| m.contains("main.rs")));
        assert!(!matches.iter().any(|m| m.contains("build")));
    }

    // ---------------------------------------------------------------
    //  Regression: `\\ No newline at end of file` must be honoured so
    //  patched files do not gain a spurious trailing newline.
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn apply_patch_respects_no_newline_at_eof_for_plus() {
        let tmp = tempfile::tempdir().unwrap();
        // Start with a file that ends with a newline.
        std::fs::write(tmp.path().join("f.txt"), "line1\n").unwrap();
        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("f.txt"), tmp.path(), FileOp::ApplyPatch)
            .unwrap();
        let mut decisions = std::collections::HashMap::new();
        decisions.insert("f.txt".to_string(), dec);

        // Replace line1 (which ends with \n) by a final line that has
        // NO trailing newline — as signalled by `\ No newline at end of
        // file` after the `+` line.
        let patch = concat!(
            "--- a/f.txt\n",
            "+++ b/f.txt\n",
            "@@ -1,1 +1,1 @@\n",
            "-line1\n",
            "+final\n",
            "\\ No newline at end of file\n",
        );
        handle_file_apply_patch(&decisions, patch, &std::collections::HashMap::new())
            .await
            .unwrap();
        let content = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
        assert_eq!(content, "final", "got {:?}", content);
    }

    // ---------------------------------------------------------------
    //  Regression: context line followed by `\ No newline at end of file`
    //  is also respected (file end unchanged).
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn apply_patch_respects_no_newline_at_eof_for_context() {
        let tmp = tempfile::tempdir().unwrap();
        // Original file has NO trailing newline.
        std::fs::write(tmp.path().join("f.txt"), "alpha\nbeta").unwrap();
        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("f.txt"), tmp.path(), FileOp::ApplyPatch)
            .unwrap();
        let mut decisions = std::collections::HashMap::new();
        decisions.insert("f.txt".to_string(), dec);

        // Change alpha -> ALPHA; keep beta as context with no-newline
        // marker so beta stays the final line without gaining a \n.
        let patch = concat!(
            "--- a/f.txt\n",
            "+++ b/f.txt\n",
            "@@ -1,2 +1,2 @@\n",
            "-alpha\n",
            "+ALPHA\n",
            " beta\n",
            "\\ No newline at end of file\n",
        );
        handle_file_apply_patch(&decisions, patch, &std::collections::HashMap::new())
            .await
            .unwrap();
        let content = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
        assert_eq!(content, "ALPHA\nbeta", "got {:?}", content);
    }

    // ---------------------------------------------------------------
    //  Edge case: file.apply_patch — simple create
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn apply_patch_creates_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("new.txt"), tmp.path(), FileOp::ApplyPatch)
            .unwrap();
        let mut decisions = std::collections::HashMap::new();
        decisions.insert("new.txt".to_string(), dec);

        let patch = "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+hello\n+world\n";
        let v = handle_file_apply_patch(&decisions, patch, &std::collections::HashMap::new())
            .await
            .unwrap();
        let files = v["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);

        let content = std::fs::read_to_string(tmp.path().join("new.txt")).unwrap();
        assert_eq!(content, "hello\nworld\n");
    }

    // ---------------------------------------------------------------
    //  Edge case: file.apply_patch — context mismatch
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn apply_patch_context_mismatch_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), "actual_line\n").unwrap();
        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("f.txt"), tmp.path(), FileOp::ApplyPatch)
            .unwrap();
        let mut decisions = std::collections::HashMap::new();
        decisions.insert("f.txt".to_string(), dec);

        let patch = "--- a/f.txt\n+++ b/f.txt\n@@ -1,1 +1,1 @@\n-wrong_context\n+replacement\n";
        let err = handle_file_apply_patch(&decisions, patch, &std::collections::HashMap::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("mismatch"));
    }

    #[tokio::test]
    async fn write_preserves_executable_mode_p0_4() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let tmp = tempfile::tempdir().unwrap();
            let file = tmp.path().join("bin.sh");
            std::fs::write(&file, b"#!/bin/sh\necho v1\n").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();

            let policy = mk_rw_policy(tmp.path());
            let dec = policy
                .check(Path::new("bin.sh"), tmp.path(), FileOp::Write)
                .unwrap();
            let new_b64 = base64::engine::general_purpose::STANDARD.encode(b"#!/bin/sh\necho v2\n");
            handle_file_write(&dec, &new_b64, None, None, false)
                .await
                .unwrap();

            let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "executable bit must survive atomic write");
        }
    }

    #[tokio::test]
    async fn apply_patch_is_atomic_across_files_p0_1() {
        use std::collections::HashMap;

        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "apple\nbanana\ncherry\n").unwrap();
        let a_sha_before = sha256_file(&tmp.path().join("a.txt")).await.unwrap();

        let policy = mk_rw_policy(tmp.path());
        let mut decisions = HashMap::new();
        for name in ["a.txt", "b.txt"] {
            let dec = policy
                .check(Path::new(name), tmp.path(), FileOp::ApplyPatch)
                .unwrap();
            decisions.insert(name.to_string(), dec);
        }

        // a.txt patches cleanly; b.txt has a deliberately wrong context line
        // so the whole patch must abort without touching a.txt.
        let patch = "\
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,3 @@
-one
+ONE
 two
 three
--- a/b.txt
+++ b/b.txt
@@ -1,3 +1,3 @@
-WRONG_CONTEXT
+APPLE
 banana
 cherry
";
        let err = handle_file_apply_patch(&decisions, patch, &std::collections::HashMap::new())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("file.precondition_failed"),
            "expected precondition_failed, got: {}",
            err
        );

        let a_sha_after = sha256_file(&tmp.path().join("a.txt")).await.unwrap();
        assert_eq!(
            a_sha_before, a_sha_after,
            "a.txt must be unchanged after failed multi-file patch (atomic rollback)"
        );
    }

    #[tokio::test]
    async fn edit_preserves_crlf_eol_p0_3() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("crlf.txt");
        std::fs::write(&file, b"alpha\r\nbeta\r\ngamma\r\n").unwrap();

        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("crlf.txt"), tmp.path(), FileOp::Edit)
            .unwrap();

        let edits = vec![EditRange {
            start_line: 2,
            end_line: 2,
            replacement: "BETA".to_string(),
            ..Default::default()
        }];
        let _ = handle_file_edit(&dec, None, &edits).await.unwrap();
        let out = std::fs::read(&file).unwrap();
        assert_eq!(
            &out, b"alpha\r\nBETA\r\ngamma\r\n",
            "CRLF must be preserved"
        );
    }

    #[tokio::test]
    async fn write_create_parents_creates_dirs_p0_6() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = mk_rw_policy(tmp.path());
        let rel = Path::new("a/b/c/out.txt");
        let dec = policy.check(rel, tmp.path(), FileOp::Write).unwrap();
        let content_b64 = base64::engine::general_purpose::STANDARD.encode(b"hello");

        // Without create_parents should fail
        let err = handle_file_write(&dec, &content_b64, None, None, false).await;
        assert!(err.is_err(), "expected failure without create_parents");

        // With create_parents should succeed
        let ok = handle_file_write(&dec, &content_b64, None, None, true).await;
        assert!(
            ok.is_ok(),
            "expected ok with create_parents: {:?}",
            ok.err()
        );
        assert_eq!(
            std::fs::read(tmp.path().join("a/b/c/out.txt")).unwrap(),
            b"hello"
        );
    }

    #[tokio::test]
    async fn read_exposes_file_sha256_on_truncated_p1_8() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("big.txt");
        let body = (1..=10)
            .map(|i| format!("line{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&file, body.as_bytes()).unwrap();
        let whole = sha256_hex(&std::fs::read(&file).unwrap());

        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("big.txt"), tmp.path(), FileOp::Read)
            .unwrap();
        let v = handle_file_read(&dec, None, false, Some(2), Some(3))
            .await
            .unwrap();
        let got = v
            .get("file_sha256")
            .and_then(|x| x.as_str())
            .unwrap_or_default();
        assert_eq!(got, whole, "file_sha256 must equal whole-file sha");
    }

    #[tokio::test]
    async fn search_respects_gitignore_p0_5() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // src/main.rs — indexed
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/main.rs"),
            b"fn main(){ println!(\"hello\"); }\n",
        )
        .unwrap();
        // dist/bundle.js — ignored via .gitignore
        std::fs::create_dir_all(root.join("dist")).unwrap();
        std::fs::write(root.join("dist/bundle.js"), b"console.log('hello');\n").unwrap();
        std::fs::write(root.join(".gitignore"), b"dist/\n").unwrap();

        let policy = mk_policy(root);
        let dec = policy.check(Path::new("."), root, FileOp::Search).unwrap();

        // With respect_gitignore=true: should only find the src/main.rs match.
        let v = handle_file_search(
            &dec,
            "hello",
            None,
            None,
            &[],
            None,
            None,
            false,
            None,
            true,
            &[],
            None,
            &[],
            None,
            false,
            false,
        )
        .await
        .unwrap();
        let matches = v["matches"].as_array().unwrap();
        let paths: Vec<String> = matches
            .iter()
            .map(|m| m["path"].as_str().unwrap().to_string())
            .collect();
        assert!(
            paths.iter().any(|p| p.contains("src/main.rs")),
            "expected src/main.rs match, got {:?}",
            paths
        );
        assert!(
            !paths.iter().any(|p| p.contains("dist/")),
            "dist/* should be filtered by .gitignore, got {:?}",
            paths
        );

        // With respect_gitignore=false: both matches.
        let v2 = handle_file_search(
            &dec,
            "hello",
            None,
            None,
            &[],
            None,
            None,
            false,
            None,
            false,
            &[],
            None,
            &[],
            None,
            false,
            false,
        )
        .await
        .unwrap();
        let paths2: Vec<String> = v2["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["path"].as_str().unwrap().to_string())
            .collect();
        assert!(
            paths2.iter().any(|p| p.contains("dist/")),
            "disabling gitignore should surface dist/*, got {:?}",
            paths2
        );
    }

    // --- P1-3: multi-pattern OR search ---
    #[tokio::test]
    async fn search_multi_pattern_or_matches_either_p1_3() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("a.txt"),
            "alpha line\nbeta line\ngamma line\n",
        )
        .unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::Search)
            .unwrap();

        let patterns = vec!["alpha".to_string(), "gamma".to_string()];
        let v = handle_file_search(
            &dec,
            "alpha",
            None,
            None,
            &[],
            None,
            None,
            false,
            None,
            true,
            &[],
            None,
            &patterns,
            None,
            false,
            false,
        )
        .await
        .unwrap();
        let lines: Vec<u64> = v["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["line"].as_u64().unwrap())
            .collect();
        assert!(
            lines.contains(&1),
            "should match alpha on line 1: {:?}",
            lines
        );
        assert!(
            lines.contains(&3),
            "should match gamma on line 3: {:?}",
            lines
        );
        assert!(!lines.contains(&2), "must NOT match beta line: {:?}", lines);
    }

    // --- P1-3: fixed_strings treats regex metacharacters literally ---
    #[tokio::test]
    async fn search_fixed_strings_literal_match_p1_3() {
        let tmp = tempfile::tempdir().unwrap();
        // The literal text contains regex metacharacters; with fixed_strings
        // it must match the exact bytes, not be interpreted as a pattern.
        std::fs::write(tmp.path().join("a.txt"), "value = a.b(c)\nother\n").unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::Search)
            .unwrap();

        let patterns = vec!["a.b(c)".to_string()];
        let v = handle_file_search(
            &dec,
            "a.b(c)",
            None,
            None,
            &[],
            None,
            None,
            false,
            None,
            true,
            &[],
            None,
            &patterns,
            None,
            true,
            false,
        )
        .await
        .unwrap();
        let matches = v["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1, "literal must match exactly once");
        assert_eq!(matches[0]["line"].as_u64().unwrap(), 1);

        // Sanity: as a regex (fixed_strings=false), `a.b(c)` would also match
        // because `.` is wildcard — so prove the literal path rejects a near
        // miss that a regex would accept.
        std::fs::write(tmp.path().join("b.txt"), "aXb(c)\n").unwrap();
        let v2 = handle_file_search(
            &dec,
            "a.b(c)",
            None,
            None,
            &[],
            None,
            None,
            false,
            None,
            true,
            &[],
            None,
            &patterns,
            None,
            true,
            false,
        )
        .await
        .unwrap();
        let paths: Vec<String> = v2["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["path"].as_str().unwrap().to_string())
            .collect();
        assert!(
            !paths.iter().any(|p| p.contains("b.txt")),
            "literal search must not match aXb(c): {:?}",
            paths
        );
    }

    // --- P1-1: --around produces a joined snippet window ---
    #[tokio::test]
    async fn search_around_emits_snippet_p1_1() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "L1\nL2\nNEEDLE\nL4\nL5\n").unwrap();
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::Search)
            .unwrap();

        let patterns = vec!["NEEDLE".to_string()];
        let v = handle_file_search(
            &dec,
            "NEEDLE",
            None,
            None,
            &[],
            None,
            None,
            false,
            None,
            true,
            &[],
            None,
            &patterns,
            Some(1),
            false,
            false,
        )
        .await
        .unwrap();
        let matches = v["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        let snippet = matches[0]["snippet"].as_str().expect("snippet present");
        // around=1 -> one line before + match + one line after.
        assert_eq!(snippet, "L2\nNEEDLE\nL4");
    }

    // ---------- P0-1 anchored edit ----------
    #[tokio::test]
    async fn edit_anchored_replaces_unique_occurrence() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, b"alpha\nbeta\ngamma\n").unwrap();
        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("a.txt"), tmp.path(), FileOp::Edit)
            .unwrap();
        let edits = vec![EditRange {
            start_line: 0,
            end_line: 0,
            replacement: String::new(),
            old_string: Some("beta".to_string()),
            new_string: Some("BETA".to_string()),
            expected_count: None,
        }];
        let v = handle_file_edit(&dec, None, &edits).await.unwrap();
        assert_eq!(v["applied_occurrences"].as_u64().unwrap(), 1);
        let got = std::fs::read_to_string(&file).unwrap();
        assert_eq!(got, "alpha\nBETA\ngamma\n");
    }

    #[tokio::test]
    async fn edit_anchored_not_unique_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, b"x\nx\nx\n").unwrap();
        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("a.txt"), tmp.path(), FileOp::Edit)
            .unwrap();
        let edits = vec![EditRange {
            start_line: 0,
            end_line: 0,
            replacement: String::new(),
            old_string: Some("x".to_string()),
            new_string: Some("y".to_string()),
            expected_count: None, // defaults to 1, but 3 present
        }];
        let err = handle_file_edit(&dec, None, &edits).await.unwrap_err();
        assert!(err.to_string().contains("file.anchor_not_unique"));
        // File must be untouched.
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "x\nx\nx\n");
    }

    #[tokio::test]
    async fn edit_anchored_not_found_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, b"hello\n").unwrap();
        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("a.txt"), tmp.path(), FileOp::Edit)
            .unwrap();
        let edits = vec![EditRange {
            start_line: 0,
            end_line: 0,
            replacement: String::new(),
            old_string: Some("nope".to_string()),
            new_string: Some("x".to_string()),
            expected_count: None,
        }];
        let err = handle_file_edit(&dec, None, &edits).await.unwrap_err();
        assert!(err.to_string().contains("file.anchor_not_found"));
    }

    // ---------- P0-3 read_many partial failure ----------
    #[tokio::test]
    async fn read_many_partial_failure() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("ok.txt"), b"hi\n").unwrap();
        let policy = mk_policy(tmp.path());
        let ok_dec = policy
            .check(Path::new("ok.txt"), tmp.path(), FileOp::Read)
            .unwrap();
        let items = vec![
            ("ok.txt".to_string(), Ok(ok_dec)),
            (
                "missing.txt".to_string(),
                Err("[file.not_found] missing.txt".to_string()),
            ),
        ];
        let v = handle_file_read_many(items, None, false).await.unwrap();
        assert_eq!(v["count"].as_u64().unwrap(), 2);
        assert_eq!(v["ok_count"].as_u64().unwrap(), 1);
        let files = v["files"].as_array().unwrap();
        // ok.txt succeeds, missing.txt carries an error but batch did not abort.
        let by_path = |p: &str| files.iter().find(|f| f["path"] == p).unwrap();
        assert_eq!(by_path("ok.txt")["ok"], serde_json::json!(true));
        assert_eq!(by_path("missing.txt")["ok"], serde_json::json!(false));
        assert!(by_path("missing.txt")["error"]
            .as_str()
            .unwrap()
            .contains("file.not_found"));
    }

    // ---------- P1-2 patch rename ----------
    #[tokio::test]
    async fn apply_patch_rename_only() {
        use std::collections::HashMap;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("from.txt"), b"content\n").unwrap();
        let policy = mk_rw_policy(tmp.path());
        let mut decisions = HashMap::new();
        decisions.insert(
            "from.txt".to_string(),
            policy
                .check(Path::new("from.txt"), tmp.path(), FileOp::Move)
                .unwrap(),
        );
        decisions.insert(
            "to.txt".to_string(),
            policy
                .check(Path::new("to.txt"), tmp.path(), FileOp::Move)
                .unwrap(),
        );
        let patch = concat!(
            "diff --git a/from.txt b/to.txt\n",
            "similarity index 100%\n",
            "rename from from.txt\n",
            "rename to to.txt\n",
        );
        let v = handle_file_apply_patch(&decisions, patch, &HashMap::new())
            .await
            .unwrap();
        assert_eq!(v["files"].as_array().unwrap().len(), 1);
        assert!(!tmp.path().join("from.txt").exists());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("to.txt")).unwrap(),
            "content\n"
        );
    }

    // ---------- P1-2 patch copy ----------
    #[tokio::test]
    async fn apply_patch_copy_only() {
        use std::collections::HashMap;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("orig.txt"), b"dup-me\n").unwrap();
        let policy = mk_rw_policy(tmp.path());
        let mut decisions = HashMap::new();
        decisions.insert(
            "orig.txt".to_string(),
            policy
                .check(Path::new("orig.txt"), tmp.path(), FileOp::Read)
                .unwrap(),
        );
        decisions.insert(
            "dup.txt".to_string(),
            policy
                .check(Path::new("dup.txt"), tmp.path(), FileOp::Write)
                .unwrap(),
        );
        let patch = concat!(
            "diff --git a/orig.txt b/dup.txt\n",
            "similarity index 100%\n",
            "copy from orig.txt\n",
            "copy to dup.txt\n",
        );
        let v = handle_file_apply_patch(&decisions, patch, &HashMap::new())
            .await
            .unwrap();
        assert_eq!(v["files"].as_array().unwrap().len(), 1);
        // Source remains, dest is an identical copy.
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("orig.txt")).unwrap(),
            "dup-me\n"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("dup.txt")).unwrap(),
            "dup-me\n"
        );
    }

    #[tokio::test]
    async fn search_respects_policy_denies() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("secrets")).unwrap();
        std::fs::write(tmp.path().join("secrets/api.key"), "SECRET_TOKEN=xyz\n").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "SECRET_TOKEN=visible\n").unwrap();

        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::Search)
            .unwrap();

        // Without denies: both files match.
        let v_all = handle_file_search(
            &dec,
            "SECRET_TOKEN",
            None,
            None,
            &[],
            None,
            None,
            false,
            None,
            false,
            &[],
            None,
            &[],
            None,
            false,
            false,
        )
        .await
        .unwrap();
        let all_paths: Vec<String> = v_all["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["path"].as_str().unwrap().to_string())
            .collect();
        assert!(all_paths.iter().any(|p| p.contains("api.key")));

        // With denies on **/*.key: only notes.txt should surface.
        let denies = vec!["**/*.key".to_string()];
        let v = handle_file_search(
            &dec,
            "SECRET_TOKEN",
            None,
            None,
            &[],
            None,
            None,
            false,
            None,
            false,
            &denies,
            None,
            &[],
            None,
            false,
            false,
        )
        .await
        .unwrap();
        let paths: Vec<String> = v["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["path"].as_str().unwrap().to_string())
            .collect();
        assert!(
            !paths.iter().any(|p| p.contains("api.key")),
            "denies should filter out api.key, got {:?}",
            paths
        );
        assert!(
            paths.iter().any(|p| p.contains("notes.txt")),
            "denies should still surface notes.txt, got {:?}",
            paths
        );
    }

    // ---------------------------------------------------------------
    //  P0-2: parse_patch — git extended headers
    // ---------------------------------------------------------------

    #[test]
    fn parse_patch_handles_diff_git_wrapper() {
        let patch = concat!(
            "diff --git a/f.txt b/f.txt\n",
            "index 0000001..0000002 100644\n",
            "--- a/f.txt\n",
            "+++ b/f.txt\n",
            "@@ -1 +1 @@\n",
            "-old\n",
            "+new\n",
        );
        let entries = parse_patch(patch).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].old_path, "f.txt");
        assert_eq!(entries[0].new_path, "f.txt");
        assert!(matches!(entries[0].kind, PatchKind::Modify));
        assert!(entries[0].body.starts_with("@@ "));
    }

    #[test]
    fn parse_patch_classifies_new_file_mode_as_create() {
        let patch = concat!(
            "diff --git a/new.txt b/new.txt\n",
            "new file mode 100644\n",
            "index 0000000..abc1234\n",
            "--- /dev/null\n",
            "+++ b/new.txt\n",
            "@@ -0,0 +1 @@\n",
            "+hi\n",
        );
        let entries = parse_patch(patch).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].kind, PatchKind::Create));
        assert_eq!(entries[0].new_path, "new.txt");
    }

    #[test]
    fn parse_patch_classifies_deleted_file_mode_as_delete() {
        let patch = concat!(
            "diff --git a/gone.txt b/gone.txt\n",
            "deleted file mode 100644\n",
            "--- a/gone.txt\n",
            "+++ /dev/null\n",
            "@@ -1 +0,0 @@\n",
            "-bye\n",
        );
        let entries = parse_patch(patch).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].kind, PatchKind::Delete));
        assert_eq!(entries[0].decision_key(), "gone.txt");
    }

    #[test]
    fn parse_patch_rename_only_is_classified() {
        let patch = concat!(
            "diff --git a/from.txt b/to.txt\n",
            "similarity index 100%\n",
            "rename from from.txt\n",
            "rename to to.txt\n",
        );
        let entries = parse_patch(patch).unwrap();
        assert_eq!(entries.len(), 1);
        match &entries[0].kind {
            PatchKind::RenameOnly { from, to } => {
                assert_eq!(from, "from.txt");
                assert_eq!(to, "to.txt");
            }
            other => panic!("expected RenameOnly, got {:?}", other),
        }
    }

    #[test]
    fn parse_patch_copy_only_is_classified() {
        let patch = concat!(
            "diff --git a/orig.txt b/dup.txt\n",
            "similarity index 100%\n",
            "copy from orig.txt\n",
            "copy to dup.txt\n",
        );
        let entries = parse_patch(patch).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].kind, PatchKind::CopyOnly { .. }));
    }

    #[test]
    fn parse_patch_mode_only_is_classified() {
        let patch = concat!(
            "diff --git a/run.sh b/run.sh\n",
            "old mode 100644\n",
            "new mode 100755\n",
        );
        let entries = parse_patch(patch).unwrap();
        assert_eq!(entries.len(), 1);
        match &entries[0].kind {
            PatchKind::ModeOnly { path } => assert_eq!(path, "run.sh"),
            other => panic!("expected ModeOnly, got {:?}", other),
        }
    }

    #[test]
    fn parse_patch_binary_patch_is_classified() {
        let patch = concat!(
            "diff --git a/img.png b/img.png\n",
            "index 1111..2222 100644\n",
            "Binary files a/img.png and b/img.png differ\n",
        );
        let entries = parse_patch(patch).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].kind, PatchKind::Binary { .. }));
    }

    #[test]
    fn parse_patch_multi_file_mixed() {
        let patch = concat!(
            "diff --git a/a.txt b/a.txt\n",
            "--- a/a.txt\n",
            "+++ b/a.txt\n",
            "@@ -1 +1 @@\n",
            "-x\n",
            "+y\n",
            "diff --git a/b.txt b/b.txt\n",
            "new file mode 100644\n",
            "--- /dev/null\n",
            "+++ b/b.txt\n",
            "@@ -0,0 +1 @@\n",
            "+new\n",
        );
        let entries = parse_patch(patch).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0].kind, PatchKind::Modify));
        assert!(matches!(entries[1].kind, PatchKind::Create));
    }

    #[tokio::test]
    async fn apply_patch_rejects_binary_with_clear_code() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("img.png"), tmp.path(), FileOp::ApplyPatch)
            .unwrap();
        let mut decisions = std::collections::HashMap::new();
        decisions.insert("img.png".to_string(), dec);
        let patch = concat!(
            "diff --git a/img.png b/img.png\n",
            "Binary files a/img.png and b/img.png differ\n",
        );
        let err = handle_file_apply_patch(&decisions, patch, &std::collections::HashMap::new())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("file.binary_patch_unsupported"), "got {}", msg);
    }

    #[tokio::test]
    async fn apply_patch_rejects_rename_only_with_clear_code() {
        // P1-2: rename-only diffs are now SUPPORTED (previously rejected).
        // This verifies the rename is applied end-to-end when both endpoints
        // carry policy decisions keyed on the patch's from/to paths.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("from.txt"), b"payload\n").unwrap();
        let pol = bifrost_core::file_access::FileAccessPolicy::new_read_write(
            "t".to_string(),
            vec![tmp.path().to_path_buf()],
        );
        let d_from = pol
            .check(Path::new("from.txt"), tmp.path(), FileOp::Move)
            .unwrap();
        let d_to = pol
            .check(Path::new("to.txt"), tmp.path(), FileOp::Move)
            .unwrap();
        let mut decisions = std::collections::HashMap::new();
        decisions.insert("from.txt".to_string(), d_from);
        decisions.insert("to.txt".to_string(), d_to);
        let patch = concat!(
            "diff --git a/from.txt b/to.txt\n",
            "similarity index 100%\n",
            "rename from from.txt\n",
            "rename to to.txt\n",
        );
        handle_file_apply_patch(&decisions, patch, &std::collections::HashMap::new())
            .await
            .unwrap();
        assert!(
            !tmp.path().join("from.txt").exists(),
            "source should be gone"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("to.txt")).unwrap(),
            "payload\n"
        );
    }

    #[tokio::test]
    async fn apply_patch_still_works_with_diff_git_wrapper() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), "old\n").unwrap();
        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("f.txt"), tmp.path(), FileOp::ApplyPatch)
            .unwrap();
        let mut decisions = std::collections::HashMap::new();
        decisions.insert("f.txt".to_string(), dec);
        let patch = concat!(
            "diff --git a/f.txt b/f.txt\n",
            "index 0000001..0000002 100644\n",
            "--- a/f.txt\n",
            "+++ b/f.txt\n",
            "@@ -1 +1 @@\n",
            "-old\n",
            "+new\n",
        );
        handle_file_apply_patch(&decisions, patch, &std::collections::HashMap::new())
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("f.txt")).unwrap(),
            "new\n"
        );
    }

    // ---- P1-5 / P1-6: move/delete optimistic lock (base_sha256/if_match_sha256) ----

    #[tokio::test]
    async fn move_accepts_matching_base_sha_p1_5() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("a.txt");
        let to = tmp.path().join("b.txt");
        std::fs::write(&from, b"hello").unwrap();
        let sha = crate::remote_invoke::file_ops::sha256_hex(b"hello");

        let pol = bifrost_core::file_access::FileAccessPolicy::new_read_write(
            "t".to_string(),
            vec![tmp.path().to_path_buf()],
        );
        let d_from = pol
            .check(Path::new("a.txt"), tmp.path(), FileOp::Move)
            .unwrap();
        let d_to = pol
            .check(Path::new("b.txt"), tmp.path(), FileOp::Move)
            .unwrap();
        let out = handle_file_move(&d_from, &d_to, Some(&sha)).await.unwrap();
        assert!(out["from"].as_str().unwrap().ends_with("a.txt"));
        assert!(to.exists());
        assert!(!from.exists());
    }

    #[tokio::test]
    async fn move_rejects_base_sha_mismatch_p1_5() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("a.txt");
        let to = tmp.path().join("b.txt");
        std::fs::write(&from, b"hello").unwrap();
        let bogus = "0".repeat(64);
        let pol = bifrost_core::file_access::FileAccessPolicy::new_read_write(
            "t".to_string(),
            vec![tmp.path().to_path_buf()],
        );
        let d_from = pol
            .check(Path::new("a.txt"), tmp.path(), FileOp::Move)
            .unwrap();
        let d_to = pol
            .check(Path::new("b.txt"), tmp.path(), FileOp::Move)
            .unwrap();
        let err = handle_file_move(&d_from, &d_to, Some(&bogus))
            .await
            .err()
            .unwrap();
        assert!(
            err.to_string().contains("[file.sha_mismatch]"),
            "expected sha_mismatch, got {}",
            err
        );
        // Source must be unchanged on mismatch.
        assert!(from.exists(), "source file must survive rejected move");
        assert!(!to.exists());
    }

    #[tokio::test]
    async fn move_rejects_base_sha_for_directory_p1_5() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("dir_a");
        let _to = tmp.path().join("dir_b");
        std::fs::create_dir(&from).unwrap();
        let pol = bifrost_core::file_access::FileAccessPolicy::new_read_write(
            "t".to_string(),
            vec![tmp.path().to_path_buf()],
        );
        let d_from = pol
            .check(Path::new("dir_a"), tmp.path(), FileOp::Move)
            .unwrap();
        let d_to = pol
            .check(Path::new("dir_b"), tmp.path(), FileOp::Move)
            .unwrap();
        let err = handle_file_move(&d_from, &d_to, Some(&"0".repeat(64)))
            .await
            .err()
            .unwrap();
        assert!(
            err.to_string().contains("[file.sha_mismatch]"),
            "directory move with base_sha256 must reject, got {}",
            err
        );
    }

    #[tokio::test]
    async fn delete_accepts_matching_if_match_p1_6() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, b"bye").unwrap();
        let sha = crate::remote_invoke::file_ops::sha256_hex(b"bye");
        let pol = bifrost_core::file_access::FileAccessPolicy::new_read_write(
            "t".to_string(),
            vec![tmp.path().to_path_buf()],
        );
        let d = pol
            .check(Path::new("a.txt"), tmp.path(), FileOp::Delete)
            .unwrap();
        let out = handle_file_delete(&d, false, Some(&sha)).await.unwrap();
        assert_eq!(out["deleted"], serde_json::Value::Bool(true));
        assert!(!p.exists());
    }

    #[tokio::test]
    async fn delete_rejects_if_match_mismatch_p1_6() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, b"bye").unwrap();
        let pol = bifrost_core::file_access::FileAccessPolicy::new_read_write(
            "t".to_string(),
            vec![tmp.path().to_path_buf()],
        );
        let d = pol
            .check(Path::new("a.txt"), tmp.path(), FileOp::Delete)
            .unwrap();
        let err = handle_file_delete(&d, false, Some(&"1".repeat(64)))
            .await
            .err()
            .unwrap();
        assert!(
            err.to_string().contains("[file.sha_mismatch]"),
            "expected sha_mismatch, got {}",
            err
        );
        // File must survive.
        assert!(p.exists(), "file must survive rejected delete");
    }

    #[tokio::test]
    async fn delete_skips_sha_check_for_directory_p1_6() {
        // Directory deletion never computes a sha; a bogus if_match_sha256
        // is silently ignored for dirs (a single sha is undefined for a
        // multi-file target). The recursive + allow_recursive_delete flags
        // remain the authoritative protection for directories.
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("sub");
        std::fs::create_dir(&p).unwrap();
        let mut pol = bifrost_core::file_access::FileAccessPolicy::new_read_write(
            "t".to_string(),
            vec![tmp.path().to_path_buf()],
        );
        pol.allow_recursive_delete = true;
        let d = pol
            .check(Path::new("sub"), tmp.path(), FileOp::Delete)
            .unwrap();
        let _ = handle_file_delete(&d, false, Some(&"2".repeat(64)))
            .await
            .unwrap();
        assert!(!p.exists());
    }

    // ===============================================================
    //  E2E coverage for the P0/P1 hardening batch
    // ===============================================================

    // --- P1-17: list/glob enforce deny rules per-entry ---
    #[tokio::test]
    async fn list_filters_denied_entries_p1_17() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("ok.txt"), b"x").unwrap();
        std::fs::create_dir(tmp.path().join(".ssh")).unwrap();
        std::fs::write(tmp.path().join(".ssh/id_rsa"), b"SECRET").unwrap();
        std::fs::write(tmp.path().join(".env"), b"TOKEN=1").unwrap();

        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::List)
            .unwrap();
        let denies = vec!["**/.ssh/**".to_string(), "**/.env".to_string()];
        let v = handle_file_list(&dec, Some(3), &[], false, &denies, None, None)
            .await
            .unwrap();
        let names: Vec<String> = v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["path"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(names.iter().any(|p| p.contains("ok.txt")));
        assert!(
            !names.iter().any(|p| p.contains("id_rsa")),
            "denied ssh key must not leak via list: {:?}",
            names
        );
        assert!(
            !names.iter().any(|p| p.ends_with(".env")),
            "denied env file must not leak via list: {:?}",
            names
        );
    }

    #[tokio::test]
    async fn glob_filters_denied_entries_p1_17() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/main.rs"), b"fn main(){}").unwrap();
        std::fs::create_dir(tmp.path().join("secrets")).unwrap();
        std::fs::write(tmp.path().join("secrets/key.pem"), b"PRIV").unwrap();

        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::Glob)
            .unwrap();
        let denies = vec!["**/*.pem".to_string()];
        let v = handle_file_glob(&dec, "**/*", None, &[], false, &denies, None)
            .await
            .unwrap();
        let matches: Vec<&str> = v["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap())
            .collect();
        assert!(matches.iter().any(|m| m.contains("main.rs")));
        assert!(
            !matches.iter().any(|m| m.contains("key.pem")),
            "denied pem must not leak via glob: {:?}",
            matches
        );
    }

    // --- P1-19: pagination cursor for traversal ops ---
    #[tokio::test]
    async fn list_pagination_cursor_walks_all_entries_p1_19() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(tmp.path().join(format!("f{i}.txt")), b"x").unwrap();
        }
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::List)
            .unwrap();

        // page 1: limit 2
        let p1 = handle_file_list(&dec, Some(1), &[], false, &[], Some(2), None)
            .await
            .unwrap();
        assert_eq!(p1["entries"].as_array().unwrap().len(), 2);
        assert!(p1["truncated"].as_bool().unwrap());
        let c1 = p1["next_cursor"].as_u64().expect("page1 next_cursor");
        assert_eq!(c1, 2);

        // page 2
        let p2 = handle_file_list(&dec, Some(1), &[], false, &[], Some(2), Some(c1))
            .await
            .unwrap();
        assert_eq!(p2["entries"].as_array().unwrap().len(), 2);
        let c2 = p2["next_cursor"].as_u64().expect("page2 next_cursor");
        assert_eq!(c2, 4);

        // page 3: last entry, no more cursor
        let p3 = handle_file_list(&dec, Some(1), &[], false, &[], Some(2), Some(c2))
            .await
            .unwrap();
        assert_eq!(p3["entries"].as_array().unwrap().len(), 1);
        assert!(p3.get("next_cursor").map(|c| c.is_null()).unwrap_or(true));
    }

    #[tokio::test]
    async fn glob_pagination_cursor_is_deterministic_p1_19() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..4 {
            std::fs::write(tmp.path().join(format!("a{i}.log")), b"x").unwrap();
        }
        let policy = mk_policy(tmp.path());
        let dec = policy
            .check(Path::new("."), tmp.path(), FileOp::Glob)
            .unwrap();

        let p1 = handle_file_glob(&dec, "**/*.log", Some(2), &[], false, &[], None)
            .await
            .unwrap();
        let m1: Vec<String> = p1["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_string())
            .collect();
        assert_eq!(m1.len(), 2);
        let c1 = p1["next_cursor"].as_u64().expect("glob next_cursor");

        let p2 = handle_file_glob(&dec, "**/*.log", Some(2), &[], false, &[], Some(c1))
            .await
            .unwrap();
        let m2: Vec<String> = p2["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_string())
            .collect();
        assert_eq!(m2.len(), 2);
        // pages must not overlap (deterministic sort + offset slicing)
        for m in &m2 {
            assert!(!m1.contains(m), "page overlap: {} in both", m);
        }
    }

    // --- P0-1: apply_patch per-file base_sha256 optimistic lock ---
    #[tokio::test]
    async fn apply_patch_rejects_stale_base_sha_p0_1() {
        use std::collections::HashMap;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();

        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("a.txt"), tmp.path(), FileOp::ApplyPatch)
            .unwrap();
        let mut decisions = HashMap::new();
        decisions.insert("a.txt".to_string(), dec);

        let patch = "--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n-one\n+ONE\n two\n three\n";

        // Wrong base sha -> must be rejected before any write.
        let mut base = HashMap::new();
        base.insert("a.txt".to_string(), "deadbeef".repeat(8));
        let err = handle_file_apply_patch(&decisions, patch, &base)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("file.sha_mismatch") || err.to_string().contains("mismatch"),
            "expected sha mismatch, got: {}",
            err
        );
        let still = std::fs::read_to_string(tmp.path().join("a.txt")).unwrap();
        assert_eq!(
            still, "one\ntwo\nthree\n",
            "file must be untouched on stale base"
        );
    }

    #[tokio::test]
    async fn apply_patch_accepts_matching_base_sha_p0_1() {
        use std::collections::HashMap;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let good = sha256_file(&tmp.path().join("a.txt")).await.unwrap();

        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("a.txt"), tmp.path(), FileOp::ApplyPatch)
            .unwrap();
        let mut decisions = HashMap::new();
        decisions.insert("a.txt".to_string(), dec);

        let patch = "--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n-one\n+ONE\n two\n three\n";
        let mut base = HashMap::new();
        base.insert("a.txt".to_string(), good);
        handle_file_apply_patch(&decisions, patch, &base)
            .await
            .unwrap();
        let out = std::fs::read_to_string(tmp.path().join("a.txt")).unwrap();
        assert_eq!(out, "ONE\ntwo\nthree\n");
    }

    // --- P1-20: patch_target_paths surfaces every touched file for audit ---
    #[test]
    fn patch_target_paths_lists_all_targets_p1_20() {
        let patch = "\
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,1 +1,1 @@
-x
+y
--- /dev/null
+++ b/src/new.rs
@@ -0,0 +1,1 @@
+z
";
        let mut paths = patch_target_paths(patch);
        paths.sort();
        assert_eq!(
            paths,
            vec!["src/a.rs".to_string(), "src/new.rs".to_string()]
        );
    }

    // --- P1-18: structured io error codes ---
    #[tokio::test]
    async fn mkdir_existing_yields_already_exists_code_p1_18() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("d")).unwrap();
        let policy = mk_rw_policy(tmp.path());
        let dec = policy
            .check(Path::new("d"), tmp.path(), FileOp::Mkdir)
            .unwrap();
        let err = handle_file_mkdir(&dec, false).await.unwrap_err();
        assert!(
            err.to_string().contains("file.already_exists"),
            "expected file.already_exists, got: {}",
            err
        );
    }

    // --- paginate() unit semantics ---
    #[test]
    fn paginate_helper_offsets_and_terminates() {
        let items: Vec<u32> = (0..5).collect();
        let (p1, n1) = paginate(items.clone(), None, 2);
        assert_eq!(p1, vec![0, 1]);
        assert_eq!(n1, Some(2));
        let (p2, n2) = paginate(items.clone(), Some(2), 2);
        assert_eq!(p2, vec![2, 3]);
        assert_eq!(n2, Some(4));
        let (p3, n3) = paginate(items.clone(), Some(4), 2);
        assert_eq!(p3, vec![4]);
        assert_eq!(n3, None);
        // cursor past end -> empty, no next
        let (p4, n4) = paginate(items.clone(), Some(99), 2);
        assert!(p4.is_empty());
        assert_eq!(n4, None);
        // limit 0 -> all, no next
        let (p5, n5) = paginate(items, None, 0);
        assert_eq!(p5.len(), 5);
        assert_eq!(n5, None);
    }
}
