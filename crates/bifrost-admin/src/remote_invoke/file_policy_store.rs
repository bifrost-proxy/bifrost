//! Per-grant file access policy store for the remote-invoke executor.
//!
//! The store holds an in-memory `HashMap<GrantId, FileAccessPolicy>` hydrated
//! from `<data-dir>/file-access.toml` at startup (best-effort). If no explicit
//! policy is configured for a grant, [`FileAccessPolicyStore::resolve`] returns
//! a default read-only policy rooted at the caller's `cwd`.
//!
//! The config file is TOML of shape:
//!
//! ```toml
//! [[grant]]
//! grant_id = "g-abc"
//! name = "my-project"
//! roots = ["/Users/eden/work/project"]
//! denies = ["**/.git/**", "**/target/**"]
//! ops = ["read", "list", "stat", "glob", "search", "hash"]
//! max_read_bytes = 2097152
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::SystemTime;

use bifrost_core::file_access::{FileAccessPolicy, FileOp};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

const CONFIG_FILE_NAME: &str = "file-access.toml";

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub(crate) struct RawConfig {
    #[serde(default, rename = "grant")]
    pub grants: Vec<RawGrantPolicy>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct RawGrantPolicy {
    pub grant_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub write_denies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ops: Vec<FileOp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_read_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_write_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub respect_gitignore: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_overwrite: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_recursive_delete: Option<bool>,
}

#[derive(Debug, Default, Clone)]
pub struct FileAccessPolicyStore {
    by_grant: HashMap<String, FileAccessPolicy>,
}

impl FileAccessPolicyStore {
    /// Empty store (no per-grant overrides). `resolve` will always return the
    /// on-demand default read-only policy.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Load from `<data-dir>/file-access.toml` with mtime-based caching.
    ///
    /// The first call parses the TOML file and caches the resulting store
    /// keyed by the file's `(size, mtime)` tuple. Subsequent calls reuse
    /// the cached store as long as the tuple is unchanged — avoiding a
    /// disk read + TOML parse on every file.* request.
    ///
    /// `save_raw_config()` bumps a shared generation so the next call
    /// re-checks mtime even if the FS reported the same timestamp (some
    /// filesystems have 1s mtime resolution).
    ///
    /// Missing file or parse errors produce an empty store plus a warning
    /// — the relay can still serve requests using the default read-only
    /// policy.
    pub fn load_default() -> Self {
        let path = default_config_path();
        load_cached(&path)
    }

    pub fn load_from(path: &Path) -> Self {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to read file-access config");
                return Self::empty();
            }
        };
        let cfg: RawConfig = match toml::from_str(&raw) {
            Ok(cfg) => cfg,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to parse file-access config");
                return Self::empty();
            }
        };
        let mut by_grant = HashMap::new();
        for g in cfg.grants {
            let ops = if g.ops.is_empty() {
                vec![
                    FileOp::Read,
                    FileOp::List,
                    FileOp::Stat,
                    FileOp::Glob,
                    FileOp::Search,
                    FileOp::Hash,
                ]
            } else {
                g.ops
            };
            let mut policy = FileAccessPolicy::new_readonly(
                g.name.unwrap_or_else(|| g.grant_id.clone()),
                g.roots,
            );
            if !g.denies.is_empty() {
                policy.denies = g.denies;
            }
            if !g.write_denies.is_empty() {
                policy.write_denies = g.write_denies;
            }
            policy.ops = ops;
            if let Some(max) = g.max_read_bytes {
                policy.max_read_bytes = max;
            }
            if let Some(max) = g.max_write_bytes {
                policy.max_write_bytes = max;
            }
            if let Some(rg) = g.respect_gitignore {
                policy.respect_gitignore = rg;
            }
            if let Some(allow) = g.allow_overwrite {
                policy.allow_overwrite = allow;
            }
            if let Some(allow) = g.allow_recursive_delete {
                policy.allow_recursive_delete = allow;
            }
            by_grant.insert(g.grant_id, policy);
        }
        debug!(count = by_grant.len(), "loaded file-access policies");
        Self { by_grant }
    }

    /// Resolve the effective policy for a grant. If no explicit override is
    /// configured, return a default read-only policy rooted at `cwd`.
    pub fn resolve(&self, grant_id: &str, cwd: &Path) -> FileAccessPolicy {
        if let Some(p) = self.by_grant.get(grant_id) {
            return p.clone();
        }
        FileAccessPolicy::new_readonly(format!("default:{}", grant_id), vec![cwd.to_path_buf()])
    }
}

/// Snapshot key: `(size, mtime)` of the TOML file, or `None` if missing.
/// Two snapshots with the same key are assumed to have identical content.
type Snapshot = Option<(u64, SystemTime)>;

#[derive(Debug, Clone, Default)]
struct CacheEntry {
    snapshot: Snapshot,
    generation: u64,
    store: FileAccessPolicyStore,
}

fn cache() -> &'static RwLock<CacheEntry> {
    static CACHE: OnceLock<RwLock<CacheEntry>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(CacheEntry::default()))
}

/// Monotonic generation counter. `save_raw_config()` bumps this so the
/// next `load_cached()` revalidates mtime even when FS resolution is
/// coarse.
fn generation() -> &'static std::sync::atomic::AtomicU64 {
    static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    &GEN
}

fn current_snapshot(path: &Path) -> Snapshot {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    Some((meta.len(), mtime))
}

fn load_cached(path: &Path) -> FileAccessPolicyStore {
    let gen_now = generation().load(std::sync::atomic::Ordering::Acquire);
    let snap_now = current_snapshot(path);

    // Fast path: read-lock and return cached clone if snapshot + generation
    // match.
    {
        let c = cache().read().unwrap();
        if c.snapshot == snap_now && c.generation == gen_now {
            return c.store.clone();
        }
    }

    // Slow path: parse and update cache under write lock.
    let store = if path.exists() {
        FileAccessPolicyStore::load_from(path)
    } else {
        FileAccessPolicyStore::empty()
    };

    let mut c = cache().write().unwrap();
    // Re-read snapshot under the write lock — another writer may have
    // already updated the cache with a fresher snapshot.
    let snap_fresh = current_snapshot(path);
    *c = CacheEntry {
        snapshot: snap_fresh,
        generation: gen_now,
        store: store.clone(),
    };
    store
}

/// Invalidate the cache. Called by `save_raw_config()` to guarantee the
/// next `load_default()` re-parses, even if the new file's mtime matches
/// the old cached mtime (coarse FS resolution).
fn invalidate_cache() {
    generation().fetch_add(1, std::sync::atomic::Ordering::AcqRel);
}

fn default_config_path() -> PathBuf {
    bifrost_storage::data_dir().join(CONFIG_FILE_NAME)
}

/// Load the raw TOML config for the HTTP API. Returns the parsed
/// `RawConfig` so the frontend can display/edit each grant entry.
pub(crate) fn load_raw_config() -> RawConfig {
    let path = default_config_path();
    if !path.exists() {
        return RawConfig::default();
    }
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to read file-access config");
            return RawConfig::default();
        }
    };
    match toml::from_str(&raw) {
        Ok(cfg) => cfg,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to parse file-access config");
            RawConfig::default()
        }
    }
}

/// Save the raw config back to `<data-dir>/file-access.toml`.
pub(crate) fn save_raw_config(config: &RawConfig) -> Result<(), String> {
    let path = default_config_path();
    let content = toml::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize file-access config: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create data directory: {e}"))?;
    }
    std::fs::write(&path, content)
        .map_err(|e| format!("Failed to write file-access config: {e}"))?;
    invalidate_cache();
    debug!(path = %path.display(), "saved file-access config");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_without_override_returns_readonly_rooted_at_cwd() {
        let store = FileAccessPolicyStore::empty();
        let tmp = std::env::temp_dir();
        let policy = store.resolve("grant-xyz", &tmp);
        assert_eq!(policy.roots, vec![tmp.clone()]);
        assert!(policy.ops.contains(&FileOp::Read));
    }

    #[test]
    fn load_from_parses_grants() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("fa.toml");
        std::fs::write(
            &cfg,
            format!(
                r#"[[grant]]
grant_id = "g-1"
name = "proj"
roots = ["{}"]
denies = ["**/.git/**"]
write_denies = ["**/*.lock"]
ops = ["read", "stat", "write"]
max_read_bytes = 1024
max_write_bytes = 2048
allow_overwrite = false
allow_recursive_delete = true
"#,
                tmp.path().to_string_lossy().replace('\\', "/")
            ),
        )
        .unwrap();
        let store = FileAccessPolicyStore::load_from(&cfg);
        let p = store.resolve("g-1", tmp.path());
        assert_eq!(p.name, "proj");
        assert_eq!(p.ops, vec![FileOp::Read, FileOp::Stat, FileOp::Write]);
        assert_eq!(p.write_denies, vec!["**/*.lock"]);
        assert_eq!(p.max_read_bytes, 1024);
        assert_eq!(p.max_write_bytes, 2048);
        assert!(!p.allow_overwrite);
        assert!(p.allow_recursive_delete);
    }

    #[test]
    fn missing_file_yields_empty_store() {
        let store = FileAccessPolicyStore::load_from(Path::new("/no/such/file.toml"));
        assert!(store.by_grant.is_empty());
    }

    #[test]
    fn load_cached_reuses_snapshot_when_mtime_unchanged() {
        // Write a TOML file, load twice via load_cached, and assert the
        // second call sees the same content. We can't directly observe
        // "parse skipped", but we can overwrite the FILE in place with
        // unrelated garbage AFTER the first load and verify the cache
        // still returns the original policy — proving no re-parse.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("fa.toml");
        std::fs::write(
            &cfg,
            format!(
                r#"[[grant]]
grant_id = "g-1"
name = "proj-original"
roots = ["{}"]
ops = ["read"]
"#,
                tmp.path().to_string_lossy().replace('\\', "/")
            ),
        )
        .unwrap();

        // First call: cold cache -> parses.
        let s1 = load_cached(&cfg);
        assert_eq!(s1.resolve("g-1", tmp.path()).name, "proj-original");

        // Overwrite file content WITHOUT touching mtime by truncating
        // and writing same-length payload rapidly. On macOS / APFS the
        // mtime may still bump; use utimes to restore it.
        let original_meta = std::fs::metadata(&cfg).unwrap();
        let original_mtime = original_meta.modified().unwrap();
        std::fs::write(
            &cfg,
            format!(
                r#"[[grant]]
grant_id = "g-1"
name = "proj-tampered"
roots = ["{}"]
ops = ["read"]
"#,
                tmp.path().to_string_lossy().replace('\\', "/")
            ),
        )
        .unwrap();
        // Best-effort restore of mtime — if this fails the test is still
        // valid (the cache-by-mtime contract is only meaningful when
        // mtime really is unchanged; a tampered mtime is out of scope
        // for the cache hit path).
        if let (Ok(()), Ok(_)) = (
            filetime_set_mtime(&cfg, original_mtime),
            std::fs::metadata(&cfg).map(|m| m.modified()),
        ) {
            let fresh_mtime = std::fs::metadata(&cfg).unwrap().modified().unwrap();
            if fresh_mtime == original_mtime {
                // Cache hit path: should still see "proj-original".
                let s2 = load_cached(&cfg);
                assert_eq!(
                    s2.resolve("g-1", tmp.path()).name,
                    "proj-original",
                    "cache hit must return the originally-parsed store when mtime is unchanged"
                );
            }
        }

        // After invalidate, the cache must re-read the file and observe
        // "proj-tampered".
        invalidate_cache();
        let s3 = load_cached(&cfg);
        assert_eq!(s3.resolve("g-1", tmp.path()).name, "proj-tampered");
    }

    // Tiny helper: set mtime without pulling the filetime crate. On unix we
    // use libc::utimes; on other platforms we fall back to a no-op error so
    // the test body skips the assertion (still covers invalidate path).
    #[cfg(unix)]
    fn filetime_set_mtime(path: &Path, t: SystemTime) -> std::io::Result<()> {
        use nix::libc;
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::other("path with nul byte"))?;
        let dur = t
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let tv = libc::timeval {
            tv_sec: dur.as_secs() as libc::time_t,
            tv_usec: dur.subsec_micros() as libc::suseconds_t,
        };
        let times = [tv, tv];
        let ret = unsafe { libc::utimes(c_path.as_ptr(), times.as_ptr()) };
        if ret == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    #[cfg(not(unix))]
    fn filetime_set_mtime(_path: &Path, _t: SystemTime) -> std::io::Result<()> {
        Err(std::io::Error::other("not supported"))
    }
}
