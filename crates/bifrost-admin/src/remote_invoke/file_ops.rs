//! Phase 1 read-only filesystem operations for the Remote Invoke File API.
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

    let mut f = fs::File::open(path)
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

    let sha256 = sha256_hex(&buf);
    let truncated = (buf.len() as u64) < total_size;
    let mtime_unix = metadata.modified().ok().and_then(system_time_to_unix);

    Ok(json!({
        "content_b64": base64::engine::general_purpose::STANDARD.encode(&buf),
        "size": buf.len() as u64,
        "total_size": total_size,
        "truncated": truncated,
        "sha256": sha256,
        "mtime_unix": mtime_unix,
    }))
}

/// `file.list` — breadth-first directory listing up to `depth` levels.
pub async fn handle_file_list(decision: &PolicyDecision, depth: Option<u32>) -> Result<Value> {
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
            for (line_idx, line) in text.lines().enumerate() {
                if let Some(m) = re.find(line) {
                    hits.push(json!({
                        "path": rel,
                        "line": (line_idx as u64) + 1,
                        "column": (m.start() as u64) + 1,
                        "preview": line.chars().take(240).collect::<String>(),
                    }));
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

/// `file.hash` — content hash. Phase 1 supports only `sha256`.
pub async fn handle_file_hash(decision: &PolicyDecision, algo: Option<&str>) -> Result<Value> {
    debug_assert_eq!(decision.op, FileOp::Hash);
    let algo = algo.unwrap_or("sha256").to_ascii_lowercase();
    if algo != "sha256" {
        return Err(BifrostError::Config(format!(
            "[file.unsupported_algo] phase 1 supports only sha256, got '{}'",
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

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::file_access::{FileAccessPolicy, FileOp};
    use std::io::Write;

    fn mk_policy(root: &Path) -> FileAccessPolicy {
        FileAccessPolicy::new_readonly("t", vec![root.to_path_buf()])
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
        let v = handle_file_read(&dec, None, false).await.unwrap();
        assert_eq!(v["size"].as_u64().unwrap(), 6);
        assert_eq!(v["truncated"].as_bool().unwrap(), false);
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
        let v = handle_file_list(&dec, Some(1)).await.unwrap();
        let entries = v["entries"].as_array().unwrap();
        assert!(entries.iter().any(|e| e["name"] == "a.txt"));
        assert!(entries.iter().any(|e| e["name"] == "sub"));
    }
}
