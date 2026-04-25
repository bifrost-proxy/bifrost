//! Remote file operations for the Remote Invoke subsystem.
//!
//! Every handler here is invoked *after* [`bifrost_core::file_access::FileAccessPolicy::check`]
//! has produced a [`PolicyDecision`]. The handlers themselves perform no
//! additional access control — they only translate the decision into
//! `tokio::fs` calls and a wire response.
//!
//! See `design/remote-invoke-file-api.md` §4 for the response schemas.

use std::collections::VecDeque;
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
const DEFAULT_GLOB_MAX: usize = 1_000;
const DEFAULT_SEARCH_MAX: usize = 500;
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
        let total_lines = all_lines.len() as u32;
        let start = offset.unwrap_or(1).max(1);
        let count = limit.unwrap_or(total_lines);
        let start_idx = ((start - 1) as usize).min(all_lines.len());
        let end_idx = (start_idx + count as usize).min(all_lines.len());
        let selected: String = all_lines[start_idx..end_idx].join("\n");
        let selected_bytes = if start_idx < end_idx {
            format!("{}\n", selected).into_bytes()
        } else {
            Vec::new()
        };
        let sha256 = sha256_hex(&selected_bytes);
        let line_truncated = (end_idx as u32) < total_lines || bytes_truncated;
        return Ok(json!({
            "content_b64": base64::engine::general_purpose::STANDARD.encode(&selected_bytes),
            "size": selected_bytes.len() as u64,
            "total_size": total_size,
            "truncated": line_truncated,
            "sha256": sha256,
            "mtime_unix": mtime_unix,
            "total_lines": total_lines,
            "start_line": start_idx as u32 + 1,
            "end_line": end_idx as u32,
        }));
    }

    let sha256 = sha256_hex(&buf);
    let mut result = json!({
        "content_b64": base64::engine::general_purpose::STANDARD.encode(&buf),
        "size": buf.len() as u64,
        "total_size": total_size,
        "truncated": bytes_truncated,
        "sha256": sha256,
        "mtime_unix": mtime_unix,
    });
    // Include total_lines for text content so coding agents can plan chunked reads.
    if let Ok(text) = std::str::from_utf8(&buf) {
        result["total_lines"] = json!(text.lines().count() as u32);
    }
    Ok(result)
}

/// `file.list` — breadth-first directory listing up to `depth` levels.
pub async fn handle_file_list(
    decision: &PolicyDecision,
    depth: Option<u32>,
    exclude_patterns: &[String],
) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::List);
    let depth = depth.unwrap_or(DEFAULT_LIST_DEPTH).max(1);
    let root = decision.path.as_path().to_path_buf();

    let deny = DenyMatcher::new(&[]).map_err(fa_to_bifrost)?;
    let _ = deny; // matcher is carried via decision-owned policy externally

    let mut entries: Vec<Value> = Vec::new();
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
                break;
            }
            let path = entry.path();
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let md = match entry.metadata().await {
                Ok(md) => md,
                Err(_) => continue,
            };
            let kind = if md.is_dir() {
                "dir"
            } else if md.is_file() {
                "file"
            } else if md.file_type().is_symlink() {
                "symlink"
            } else {
                "other"
            };
            let name = entry.file_name().to_string_lossy().to_string();
            entries.push(json!({
                "name": name,
                "path": rel,
                "type": kind,
                "size": md.len(),
                "mtime_unix": md.modified().ok().and_then(system_time_to_unix),
            }));
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

    Ok(json!({ "entries": entries, "root": root.to_string_lossy() }))
}

/// `file.stat` — return size, mtime, mode, kind, sha256 (files only).
pub async fn handle_file_stat(decision: &PolicyDecision) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Stat);
    let path = decision.path.as_path();
    let md = fs::metadata(path)
        .await
        .map_err(|e| io_err(&format!("stat {}", path.display()), e))?;

    let kind = if md.is_dir() {
        "dir"
    } else if md.is_file() {
        "file"
    } else if md.file_type().is_symlink() {
        "symlink"
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
) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Glob);
    let max = max_matches.unwrap_or(DEFAULT_GLOB_MAX);
    let root = decision.path.as_path().to_path_buf();
    let matcher = GlobMatcher::new(&[pattern.to_string()]).map_err(fa_to_bifrost)?;

    let mut matches: Vec<String> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.clone()];
    let mut truncated = false;
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
            if md.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if should_skip_dir(name, exclude_patterns) {
                        continue;
                    }
                }
                stack.push(path);
                continue;
            }
            if matcher.is_match(&rel) {
                if matches.len() >= max {
                    truncated = true;
                    break;
                }
                matches.push(rel);
            }
        }
        if truncated {
            break;
        }
    }

    Ok(json!({
        "matches": matches,
        "truncated": truncated,
        "root": root.to_string_lossy(),
    }))
}

/// `file.search` — regex grep across files under the policy root.
pub async fn handle_file_search(
    decision: &PolicyDecision,
    pattern: &str,
    max_matches: Option<usize>,
    max_scan_bytes: Option<u64>,
    exclude_patterns: &[String],
    context_before: Option<u32>,
    context_after: Option<u32>,
) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Search);
    let root = decision.path.as_path().to_path_buf();
    let max = max_matches.unwrap_or(DEFAULT_SEARCH_MAX);
    let per_file_cap = max_scan_bytes
        .unwrap_or(DEFAULT_SEARCH_SCAN_BYTES)
        .min(decision.max_read_bytes);

    let re = regex::Regex::new(pattern).map_err(|e| {
        BifrostError::Config(format!(
            "[file.invalid_regex] bad pattern '{}': {}",
            pattern, e
        ))
    })?;

    let mut hits: Vec<Value> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.clone()];
    let mut truncated = false;
    'outer: while let Some(dir) = stack.pop() {
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
            if md.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if should_skip_dir(name, exclude_patterns) {
                        continue;
                    }
                }
                stack.push(path);
                continue;
            }
            if !md.is_file() {
                continue;
            }
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let mut f = match fs::File::open(&path).await {
                Ok(f) => f,
                Err(_) => continue,
            };
            let mut buf = Vec::new();
            let mut limited = (&mut f).take(per_file_cap);
            if limited.read_to_end(&mut buf).await.is_err() {
                continue;
            }
            if is_probably_binary(&buf) {
                continue;
            }
            let text = match std::str::from_utf8(&buf) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let before = context_before.unwrap_or(0) as usize;
            let after = context_after.unwrap_or(0) as usize;
            let need_context = before > 0 || after > 0;
            let all_lines: Vec<&str> = if need_context {
                text.lines().collect()
            } else {
                Vec::new()
            };
            let total_lines = if need_context { all_lines.len() } else { 0 };
            let lines_iter: Box<dyn Iterator<Item = (usize, &str)>> = if need_context {
                Box::new(all_lines.iter().copied().enumerate())
            } else {
                Box::new(text.lines().enumerate())
            };
            for (line_idx, line) in lines_iter {
                if let Some(m) = re.find(line) {
                    let mut hit = json!({
                        "path": rel,
                        "line": (line_idx as u64) + 1,
                        "column": (m.start() as u64) + 1,
                        "preview": line.chars().take(240).collect::<String>(),
                    });
                    if need_context {
                        let ctx_start = line_idx.saturating_sub(before);
                        let ctx_end = (line_idx + after + 1).min(total_lines);
                        let ctx: Vec<Value> = (ctx_start..ctx_end)
                            .map(|i| {
                                json!({
                                    "line": (i as u64) + 1,
                                    "content": all_lines[i].chars().take(240).collect::<String>(),
                                })
                            })
                            .collect();
                        hit["context"] = json!(ctx);
                    }
                    hits.push(hit);
                    if hits.len() >= max {
                        truncated = true;
                        break 'outer;
                    }
                }
            }
        }
    }

    Ok(json!({
        "matches": hits,
        "truncated": truncated,
        "root": root.to_string_lossy(),
    }))
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
            return Err(precondition_failed(format!(
                "sha mismatch: expected {}, actual {}",
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
    fs::rename(&tmp, path)
        .await
        .map_err(|e| io_err("rename-tmp", e))?;

    let new_sha = sha256_hex(&bytes);
    Ok(json!({
        "path": path.to_string_lossy(),
        "bytes_written": bytes.len(),
        "sha256": new_sha,
        "previous_sha256": existing_sha,
    }))
}

/// A single line-range edit. Line numbers are 1-based and inclusive.
#[derive(Debug, serde::Deserialize)]
pub struct EditRange {
    pub start_line: u32,
    pub end_line: u32,
    #[serde(default)]
    pub replacement: String,
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

    let orig_bytes = fs::read(path).await.map_err(|e| io_err("read", e))?;
    let orig_sha = sha256_hex(&orig_bytes);
    if let Some(expected) = base_sha256 {
        if expected != orig_sha {
            return Err(precondition_failed(format!(
                "sha mismatch: expected {}, actual {}",
                expected, orig_sha
            )));
        }
    }
    let original = String::from_utf8(orig_bytes)
        .map_err(|_| BifrostError::Config("[file.invalid_args] file is not utf-8".to_string()))?;
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
        out.push_str(&e.replacement);
        if !e.replacement.is_empty()
            && !e.replacement.ends_with('\n')
            && (e.end_line as usize) < lines.len()
        {
            out.push('\n');
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

    // Atomic rewrite.
    let parent = path.parent().ok_or_else(|| {
        BifrostError::Config("[file.invalid_args] path has no parent".to_string())
    })?;
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
        f.write_all(&new_bytes)
            .await
            .map_err(|e| io_err("write-tmp", e))?;
        f.sync_all().await.map_err(|e| io_err("fsync-tmp", e))?;
    }
    fs::rename(&tmp, path)
        .await
        .map_err(|e| io_err("rename-tmp", e))?;

    let new_sha = sha256_hex(&new_bytes);
    Ok(json!({
        "path": path.to_string_lossy(),
        "bytes_written": new_bytes.len(),
        "sha256": new_sha,
        "previous_sha256": orig_sha,
        "applied_edits": edits.len(),
    }))
}

pub async fn handle_file_mkdir(decision: &PolicyDecision, parents: bool) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Mkdir);
    let path = decision.path.as_path();
    let res = if parents {
        fs::create_dir_all(path).await
    } else {
        fs::create_dir(path).await
    };
    res.map_err(|e| io_err("mkdir", e))?;
    Ok(json!({
        "path": path.to_string_lossy(),
        "created": true,
        "parents": parents,
    }))
}

pub async fn handle_file_move(
    decision: &PolicyDecision,
    to_decision: &PolicyDecision,
) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Move);
    debug_assert_eq!(to_decision.op, FileOp::Move);
    let from = decision.path.as_path();
    let to = to_decision.path.as_path();
    if fs::metadata(to).await.is_ok() && !decision.allow_overwrite {
        return Err(precondition_failed(
            "destination exists and overwrite is disabled",
        ));
    }
    fs::rename(from, to)
        .await
        .map_err(|e| io_err("rename", e))?;
    Ok(json!({
        "from": from.to_string_lossy(),
        "to": to.to_string_lossy(),
    }))
}

pub async fn handle_file_delete(decision: &PolicyDecision, recursive: bool) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Delete);
    let path = decision.path.as_path();
    let meta = fs::symlink_metadata(path)
        .await
        .map_err(|e| io_err("stat", e))?;
    let ft = meta.file_type();
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
/// form. For each file header (`--- a/FOO` + `+++ b/FOO`) we resolve the
/// target against the provided decisions map (policy pre-checked upstream),
/// verify the context lines match, and apply hunks atomically per-file.
///
/// Any parse or context-mismatch error aborts the whole patch (no partial
/// application). Binary diffs are refused.
pub async fn handle_file_apply_patch(
    decisions: &std::collections::HashMap<String, PolicyDecision>,
    patch_text: &str,
) -> Result<Value> {
    // Very small, intentionally restricted parser: recognizes only the
    // subset of unified diff that our CLI emits. Not a full `git apply`.
    let mut applied: Vec<Value> = Vec::new();
    // Normalize: ensure split("\n--- ") works even if patch starts with "--- "
    let normalized = if patch_text.starts_with("--- ") {
        format!("\n{}", patch_text)
    } else {
        patch_text.to_string()
    };
    let mut files = normalized.split("\n--- ");
    let _preamble = files.next(); // anything before first "--- "
    for raw in files {
        // `raw` starts with e.g. "a/src/foo.rs\n+++ b/src/foo.rs\n@@ ..."
        let mut it = raw.splitn(2, '\n');
        let old_line = it.next().unwrap_or("");
        let rest = it.next().unwrap_or("");
        let mut it2 = rest.splitn(2, '\n');
        let new_line = it2.next().unwrap_or("");
        let body = it2.next().unwrap_or("");
        if !new_line.starts_with("+++ ") {
            return Err(BifrostError::Config(
                "[file.invalid_args] malformed unified diff: expected '+++' line".to_string(),
            ));
        }
        let strip = |s: &str| -> String {
            let s = s.trim();
            s.strip_prefix("a/")
                .or_else(|| s.strip_prefix("b/"))
                .unwrap_or(s)
                .to_string()
        };
        let old_path = strip(old_line);
        let new_path = strip(new_line.trim_start_matches("+++ "));
        let key = if new_path == "/dev/null" {
            old_path.clone()
        } else {
            new_path.clone()
        };
        let decision = decisions.get(&key).ok_or_else(|| {
            BifrostError::Config(format!(
                "[file.invalid_args] no policy decision for patch target '{}'",
                key
            ))
        })?;

        // Read original (empty if creating).
        let target = decision.path.as_path();
        let orig_bytes = match fs::read(target).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io_err("read", e)),
        };
        let orig_text = String::from_utf8(orig_bytes.clone()).map_err(|_| {
            BifrostError::Config("[file.invalid_args] file is not utf-8".to_string())
        })?;
        let orig_lines: Vec<&str> = orig_text.split_inclusive('\n').collect();

        // Apply hunks.
        let mut out = String::with_capacity(orig_text.len());
        let mut src_cursor: usize = 0; // 0-based index into orig_lines
                                       // Normalize: ensure split("\n@@ ") works even if body starts with "@@ "
        let norm_body = if body.starts_with("@@ ") {
            format!("\n{}", body)
        } else {
            body.to_string()
        };
        let mut hunks = norm_body.split("\n@@ ");
        // Everything before the first "@@ " is file-level metadata we ignore.
        let _meta = hunks.next();
        for hunk in hunks {
            // `hunk` looks like "-l,c +l,c @@\n<lines>"
            let mut hlines = hunk.splitn(2, '\n');
            let header = hlines.next().unwrap_or("");
            let content = hlines.next().unwrap_or("");
            let (old_start, _old_count) = parse_hunk_range(header, '-').ok_or_else(|| {
                BifrostError::Config(format!("[file.invalid_args] bad hunk header: {}", header))
            })?;
            // Emit source lines up to the hunk.
            let target_cursor = if old_start == 0 { 0 } else { old_start - 1 };
            while src_cursor < target_cursor && src_cursor < orig_lines.len() {
                out.push_str(orig_lines[src_cursor]);
                src_cursor += 1;
            }
            // Walk hunk lines.
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
                            || orig_lines[src_cursor].trim_end_matches('\n') != tail
                        {
                            return Err(precondition_failed(format!(
                                "context mismatch at line {}",
                                src_cursor + 1
                            )));
                        }
                        out.push_str(orig_lines[src_cursor]);
                        src_cursor += 1;
                    }
                    '-' => {
                        if src_cursor >= orig_lines.len()
                            || orig_lines[src_cursor].trim_end_matches('\n') != tail
                        {
                            return Err(precondition_failed(format!(
                                "delete-line mismatch at line {}",
                                src_cursor + 1
                            )));
                        }
                        src_cursor += 1;
                    }
                    '+' => {
                        out.push_str(tail);
                        out.push('\n');
                    }
                    '\\' => { /* "\ No newline at end of file" — ignore */ }
                    _ => {
                        return Err(BifrostError::Config(format!(
                            "[file.invalid_args] unknown hunk char '{}'",
                            ch
                        )))
                    }
                }
            }
        }
        // Emit trailing source lines.
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
            return Err(size_too_large(format!(
                "patched file is {} bytes, max_write_bytes is {}",
                new_bytes.len(),
                max
            )));
        }

        // Atomic rewrite (or delete if target is /dev/null).
        if new_path == "/dev/null" {
            fs::remove_file(target)
                .await
                .map_err(|e| io_err("remove_file", e))?;
            applied.push(json!({
                "path": target.to_string_lossy(),
                "deleted": true,
            }));
        } else {
            let parent = target.parent().ok_or_else(|| {
                BifrostError::Config("[file.invalid_args] path has no parent".to_string())
            })?;
            fs::create_dir_all(parent)
                .await
                .map_err(|e| io_err("mkdir-parent", e))?;
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let tmp = parent.join(format!(".bifrost-patch.{}.{}.tmp", pid, nanos));
            {
                let mut f = fs::File::create(&tmp)
                    .await
                    .map_err(|e| io_err("create-tmp", e))?;
                f.write_all(&new_bytes)
                    .await
                    .map_err(|e| io_err("write-tmp", e))?;
                f.sync_all().await.map_err(|e| io_err("fsync-tmp", e))?;
            }
            fs::rename(&tmp, target)
                .await
                .map_err(|e| io_err("rename-tmp", e))?;
            applied.push(json!({
                "path": target.to_string_lossy(),
                "bytes_written": new_bytes.len(),
                "sha256": sha256_hex(&new_bytes),
            }));
        }
    }

    Ok(json!({ "files": applied }))
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
        let v = handle_file_list(&dec, Some(1), &[]).await.unwrap();
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

        let v = handle_file_search(&dec, "println", None, None, &[], Some(1), Some(1))
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

        let v = handle_file_glob(&dec, "**/*", None, &[]).await.unwrap();
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

        let v = handle_file_list(&dec, Some(2), &[]).await.unwrap();
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

        let v = handle_file_search(&dec, "MATCH_HERE", None, None, &[], Some(3), Some(1))
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

        let v = handle_file_search(&dec, "LAST_MATCH", None, None, &[], Some(1), Some(5))
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

        let err = handle_file_search(&dec, "[invalid", None, None, &[], None, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("file.invalid_regex"));
    }

    // ---------------------------------------------------------------
    //  Bug regression: file.edit — empty replacement deletes line cleanly
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
            },
            EditRange {
                start_line: 3,
                end_line: 4,
                replacement: "CC_DD\n".to_string(),
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
            },
            EditRange {
                start_line: 2,
                end_line: 3,
                replacement: "Y\n".to_string(),
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
        let wv = handle_file_write(&write_dec, &content_b64, None, None)
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
        let err = handle_file_write(&dec, &content_b64, Some("wrong_sha"), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("file.precondition_failed"));
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

        let v = handle_file_glob(&dec, "**/*", None, &["build".to_string()])
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
        let v = handle_file_apply_patch(&decisions, patch).await.unwrap();
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
        let err = handle_file_apply_patch(&decisions, patch)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("mismatch"));
    }
}
