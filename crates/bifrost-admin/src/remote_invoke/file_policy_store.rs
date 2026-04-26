//! Per-grant file access policy store for the remote-invoke executor.
//!
//! The store hydrates a list of grant-policy entries from
//! `<data-dir>/file-access.toml` at startup (best-effort) and resolves the
//! effective [`FileAccessPolicy`] for each incoming `file.*` call using a
//! deterministic match-priority chain:
//!
//! 1. Exact `match.grant_id` match (or legacy flat `grant_id = "..."`)
//! 2. `match.ssh_fingerprint` match (relay-wide SSH key pinning)
//! 3. `match.caller_fingerprint` match (caller ephemeral key pinning)
//! 4. The `[default]` section, if present
//! 5. A hardcoded read-only policy rooted at the caller's `cwd`
//!
//! Within each tier, entries are evaluated in file order: the first
//! matching entry wins. This means more-specific grants should be listed
//! earlier in `file-access.toml`.
//!
//! The config file is TOML of shape:
//!
//! ```toml
//! [[grant]]
//! match.ssh_fingerprint = "5f02477..."
//! name = "my macbook via ssh key"
//! roots = ["/Users/eden"]
//! ops = ["read", "list", "stat", "glob", "search", "hash",
//!        "write", "edit", "mkdir", "move", "delete", "apply_patch"]
//!
//! [[grant]]
//! match.grant_id = "7e5e03e0-..."       # pinning by exact grant id still works
//! roots = ["/Users/eden/work"]
//! ops = ["read", "list", "stat"]
//!
//! [[grant]]
//! grant_id = "legacy-id"                # legacy flat field, still accepted
//! roots = ["/tmp/legacy"]
//! ops = ["read"]
//!
//! [default]
//! roots = ["/Users/eden"]
//! ops = ["read", "list", "stat", "glob", "search", "hash"]
//! ```
//!
//! See `resolve()` for the precise lookup order.

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<RawDefaultPolicy>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct GrantMatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct RawGrantPolicy {
    #[serde(default, rename = "match")]
    pub match_: GrantMatch,
    /// Legacy flat field. If `match.grant_id` is empty but this is set we
    /// fold it into `match.grant_id` at load time so downstream code only
    /// needs to consult `match_`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
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

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub(crate) struct RawDefaultPolicy {
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
    /// Entries preserve TOML file order so callers can rely on
    /// "first match wins" semantics within each match tier.
    entries: Vec<(GrantMatch, FileAccessPolicy)>,
    default_policy: Option<FileAccessPolicy>,
}

impl FileAccessPolicyStore {
    /// Empty store (no entries, no default). `resolve` falls back to the
    /// hardcoded read-only cwd policy.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Load from `<data-dir>/file-access.toml` with mtime-based caching.
    ///
    /// See module docs for the match-priority chain. Missing file or
    /// parse errors produce an empty store plus a warning — the relay
    /// can still serve requests using the default read-only policy.
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
        Self::from_raw(cfg)
    }

    pub(crate) fn from_raw(cfg: RawConfig) -> Self {
        let mut entries: Vec<(GrantMatch, FileAccessPolicy)> = Vec::with_capacity(cfg.grants.len());
        for mut g in cfg.grants {
            // Fold legacy flat `grant_id` into `match.grant_id`.
            if g.match_.grant_id.is_none() {
                if let Some(legacy) = g.grant_id.take() {
                    g.match_.grant_id = Some(legacy);
                }
            }

            let display_name = g
                .name
                .clone()
                .or_else(|| g.match_.grant_id.clone())
                .or_else(|| g.match_.ssh_fingerprint.clone())
                .or_else(|| g.match_.caller_fingerprint.clone())
                .unwrap_or_else(|| "file-access".to_string());

            let ops = default_read_ops_if_empty(g.ops);
            let mut policy = FileAccessPolicy::new_readonly(display_name, g.roots);
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
            entries.push((g.match_, policy));
        }

        let default_policy = cfg.default.map(|d| {
            let ops = default_read_ops_if_empty(d.ops);
            let mut policy = FileAccessPolicy::new_readonly("default".to_string(), d.roots);
            if !d.denies.is_empty() {
                policy.denies = d.denies;
            }
            if !d.write_denies.is_empty() {
                policy.write_denies = d.write_denies;
            }
            policy.ops = ops;
            if let Some(max) = d.max_read_bytes {
                policy.max_read_bytes = max;
            }
            if let Some(max) = d.max_write_bytes {
                policy.max_write_bytes = max;
            }
            if let Some(rg) = d.respect_gitignore {
                policy.respect_gitignore = rg;
            }
            if let Some(allow) = d.allow_overwrite {
                policy.allow_overwrite = allow;
            }
            if let Some(allow) = d.allow_recursive_delete {
                policy.allow_recursive_delete = allow;
            }
            policy
        });

        debug!(
            count = entries.len(),
            has_default = default_policy.is_some(),
            "loaded file-access policies"
        );
        Self {
            entries,
            default_policy,
        }
    }

    /// Resolve the effective policy for a call using the match-priority
    /// chain documented at the module level. Within each tier, entries
    /// are scanned in TOML file order and the first match wins.
    pub fn resolve(
        &self,
        grant_id: &str,
        caller_fingerprint: Option<&str>,
        ssh_fingerprint: Option<&str>,
        cwd: &Path,
    ) -> FileAccessPolicy {
        // 1. Exact grant_id.
        for (m, p) in &self.entries {
            if m.grant_id.as_deref() == Some(grant_id) {
                return p.clone();
            }
        }
        // 2. ssh_fingerprint.
        if let Some(ssh_fp) = ssh_fingerprint {
            for (m, p) in &self.entries {
                if m.ssh_fingerprint.as_deref() == Some(ssh_fp) {
                    return p.clone();
                }
            }
        }
        // 3. caller_fingerprint.
        if let Some(cf) = caller_fingerprint {
            for (m, p) in &self.entries {
                if m.caller_fingerprint.as_deref() == Some(cf) {
                    return p.clone();
                }
            }
        }
        // 4. [default] section.
        if let Some(p) = &self.default_policy {
            return p.clone();
        }
        // 5. Hardcoded read-only rooted at cwd.
        FileAccessPolicy::new_readonly(format!("default:{grant_id}"), vec![cwd.to_path_buf()])
    }
}

fn default_read_ops_if_empty(ops: Vec<FileOp>) -> Vec<FileOp> {
    if ops.is_empty() {
        vec![
            FileOp::Read,
            FileOp::List,
            FileOp::Stat,
            FileOp::Glob,
            FileOp::Search,
            FileOp::Hash,
        ]
    } else {
        ops
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

    fn parse(toml_str: &str) -> FileAccessPolicyStore {
        let cfg: RawConfig = toml::from_str(toml_str).expect("toml parses");
        FileAccessPolicyStore::from_raw(cfg)
    }

    #[test]
    fn resolves_by_exact_grant_id() {
        let store = parse(
            r#"
[[grant]]
match.grant_id = "g-alpha"
name = "alpha"
roots = ["/tmp/alpha"]
ops = ["read", "write"]

[[grant]]
match.grant_id = "g-beta"
name = "beta"
roots = ["/tmp/beta"]
ops = ["read"]
"#,
        );
        let cwd = Path::new("/tmp");
        let p = store.resolve("g-beta", None, None, cwd);
        assert_eq!(p.name, "beta");
        assert_eq!(p.roots, vec![PathBuf::from("/tmp/beta")]);
    }

    #[test]
    fn resolves_by_ssh_fingerprint_when_grant_id_miss() {
        let store = parse(
            r#"
[[grant]]
match.ssh_fingerprint = "ssh-fp-xyz"
name = "via-ssh"
roots = ["/users/eden"]
ops = ["read", "list", "write"]
"#,
        );
        let cwd = Path::new("/tmp");
        let p = store.resolve("unknown-grant", None, Some("ssh-fp-xyz"), cwd);
        assert_eq!(p.name, "via-ssh");
        assert!(p.ops.contains(&FileOp::Write));
    }

    #[test]
    fn resolves_by_caller_fingerprint_when_ssh_miss() {
        let store = parse(
            r#"
[[grant]]
match.caller_fingerprint = "caller-fp-abc"
name = "via-caller"
roots = ["/home/user"]
ops = ["read"]
"#,
        );
        let cwd = Path::new("/tmp");
        let p = store.resolve("unknown-grant", Some("caller-fp-abc"), None, cwd);
        assert_eq!(p.name, "via-caller");
    }

    #[test]
    fn default_section_applies_when_all_match_miss() {
        let store = parse(
            r#"
[[grant]]
match.grant_id = "g-specific"
name = "specific"
roots = ["/specific"]

[default]
roots = ["/fallback"]
ops = ["read", "list", "stat"]
"#,
        );
        let cwd = Path::new("/tmp");
        let p = store.resolve("unrelated-grant", Some("x"), Some("y"), cwd);
        assert_eq!(p.name, "default");
        assert_eq!(p.roots, vec![PathBuf::from("/fallback")]);
        assert!(p.ops.contains(&FileOp::Read));
    }

    #[test]
    fn hardcoded_readonly_when_no_default_and_no_match() {
        let store = FileAccessPolicyStore::empty();
        let cwd = Path::new("/var/tmp/scratch");
        let p = store.resolve("any-grant", None, None, cwd);
        assert_eq!(p.name, "default:any-grant");
        assert_eq!(p.roots, vec![cwd.to_path_buf()]);
        assert!(p.ops.contains(&FileOp::Read));
        // Read-only: no write ops.
        assert!(!p.ops.contains(&FileOp::Write));
    }

    #[test]
    fn legacy_flat_grant_id_still_works() {
        let store = parse(
            r#"
[[grant]]
grant_id = "legacy-g-1"
name = "legacy"
roots = ["/legacy"]
ops = ["read"]
"#,
        );
        let cwd = Path::new("/tmp");
        let p = store.resolve("legacy-g-1", None, None, cwd);
        assert_eq!(p.name, "legacy");
        assert_eq!(p.roots, vec![PathBuf::from("/legacy")]);
    }

    #[test]
    fn grant_id_precedence_over_fingerprint() {
        // Two separate entries: one matches by ssh_fingerprint with a
        // permissive policy, a LATER one matches by grant_id with a
        // restrictive policy. The grant_id match should win because tier 1
        // (grant_id) beats tier 2 (ssh_fingerprint) regardless of order.
        let store = parse(
            r#"
[[grant]]
match.ssh_fingerprint = "shared-ssh"
name = "ssh-wide"
roots = ["/ssh-wide"]
ops = ["read", "write"]

[[grant]]
match.grant_id = "g-narrow"
name = "narrow"
roots = ["/narrow"]
ops = ["read"]
"#,
        );
        let cwd = Path::new("/tmp");
        let p = store.resolve("g-narrow", None, Some("shared-ssh"), cwd);
        assert_eq!(p.name, "narrow");
        assert!(!p.ops.contains(&FileOp::Write));
    }

    #[test]
    fn match_priority_order_deterministic() {
        // Multiple entries all match by ssh_fingerprint — first in file
        // order wins.
        let store = parse(
            r#"
[[grant]]
match.ssh_fingerprint = "dup-fp"
name = "first"
roots = ["/first"]
ops = ["read"]

[[grant]]
match.ssh_fingerprint = "dup-fp"
name = "second"
roots = ["/second"]
ops = ["read", "write"]
"#,
        );
        let cwd = Path::new("/tmp");
        let p = store.resolve("unknown", None, Some("dup-fp"), cwd);
        assert_eq!(p.name, "first");
        assert_eq!(p.roots, vec![PathBuf::from("/first")]);
    }

    // --- Legacy tests preserved below ---

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
        let p = store.resolve("g-1", None, None, tmp.path());
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
        assert!(store.entries.is_empty());
        assert!(store.default_policy.is_none());
    }

    #[test]
    fn load_cached_reuses_snapshot_when_mtime_unchanged() {
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
        assert_eq!(
            s1.resolve("g-1", None, None, tmp.path()).name,
            "proj-original"
        );

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
        if let (Ok(()), Ok(_)) = (
            filetime_set_mtime(&cfg, original_mtime),
            std::fs::metadata(&cfg).map(|m| m.modified()),
        ) {
            let fresh_mtime = std::fs::metadata(&cfg).unwrap().modified().unwrap();
            if fresh_mtime == original_mtime {
                let s2 = load_cached(&cfg);
                assert_eq!(
                    s2.resolve("g-1", None, None, tmp.path()).name,
                    "proj-original",
                    "cache hit must return the originally-parsed store when mtime is unchanged"
                );
            }
        }

        invalidate_cache();
        let s3 = load_cached(&cfg);
        assert_eq!(
            s3.resolve("g-1", None, None, tmp.path()).name,
            "proj-tampered"
        );
    }

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
