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
//! roots = ["/home/user"]
//! ops = ["read", "list", "stat", "glob", "search", "hash",
//!        "write", "edit", "mkdir", "move", "delete", "apply_patch"]
//!
//! [[grant]]
//! match.grant_id = "7e5e03e0-..."       # pinning by exact grant id still works
//! roots = ["/home/user/work"]
//! ops = ["read", "list", "stat"]
//!
//! [[grant]]
//! grant_id = "legacy-id"                # legacy flat field, still accepted
//! roots = ["/tmp/legacy"]
//! ops = ["read"]
//!
//! [default]
//! roots = ["/home/user"]
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
pub(crate) struct GrantMatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_fingerprint: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
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
        const MAX_STORE_FILE_BYTES: u64 = 256 * 1024 * 1024;
        if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > MAX_STORE_FILE_BYTES {
            warn!(path = %path.display(), "file-access config too large, using empty config");
            return Self::empty();
        }
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

    /// Build a store from an already-parsed config.
    ///
    /// # Fingerprint trust root
    ///
    /// The caller/ssh fingerprints used by [`Self::resolve`] are NOT
    /// taken from the remote-invoke wire payload. They are resolved
    /// locally by the worker from the `local_grants` admin table
    /// (see `worker.rs` where `caller_fp` / `ssh_fp` are written into
    /// `RemoteCommand` and marked `#[serde(skip)]` on the type).
    ///
    /// This means a malicious peer cannot spoof a fingerprint match:
    /// the fingerprints this store sees come from grants the local
    /// user explicitly approved on this device. `from_raw` itself is
    /// trust-agnostic — it just materializes whatever the operator
    /// put in `file-access.toml` — but the overall match chain is
    /// only sound because of this upstream binding. Keep that
    /// invariant in mind when refactoring callers.
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

/// Full set of 12 file operations. SSH-key authorization seeds a grant
/// with this set by default so coding-agent flows can use file.write /
/// file.edit / file.patch / etc. without extra manual TOML edits.
pub fn full_file_ops() -> Vec<FileOp> {
    vec![
        FileOp::Read,
        FileOp::List,
        FileOp::Stat,
        FileOp::Glob,
        FileOp::Search,
        FileOp::Hash,
        FileOp::Write,
        FileOp::Edit,
        FileOp::Mkdir,
        FileOp::Move,
        FileOp::Delete,
        FileOp::ApplyPatch,
    ]
}

/// Insert or update a `[[grant]]` entry keyed by `match.ssh_fingerprint`.
///
/// Semantics:
/// - Idempotent: if an existing `[[grant]]` has the same
///   `match.ssh_fingerprint`, its fields (`roots`, `ops`, `name`, flags) are
///   overwritten; all other entries and the `[default]` section are preserved
///   byte-for-byte.
/// - Callers that want to *preserve* a user's manual edits should check for
///   the presence of a matching entry before calling this function. The
///   default use site (`worker::create_ssh_key`) is a fresh key creation, so
///   overwriting is the desired behavior.
/// - The cache is invalidated, so the next `load_default()` will re-parse.
pub fn upsert_ssh_fingerprint_grant(
    fingerprint: &str,
    name: Option<String>,
    roots: Vec<PathBuf>,
    ops: Vec<FileOp>,
    allow_overwrite: Option<bool>,
    allow_recursive_delete: Option<bool>,
) -> Result<(), String> {
    let mut cfg = load_raw_config();
    merge_ssh_fingerprint_grant_in_place(
        &mut cfg,
        fingerprint,
        name,
        roots,
        ops,
        allow_overwrite,
        allow_recursive_delete,
    );
    save_raw_config(&cfg)
}

pub fn has_ssh_fingerprint_grant(fingerprint: &str) -> bool {
    raw_config_has_ssh_fingerprint_grant(&load_raw_config(), fingerprint)
}

pub(crate) fn raw_config_has_ssh_fingerprint_grant(cfg: &RawConfig, fingerprint: &str) -> bool {
    cfg.grants
        .iter()
        .any(|g| g.match_.ssh_fingerprint.as_deref() == Some(fingerprint))
}

/// Remove `[[grant]]` entries that are pinned to an exact grant id.
///
/// This is called when a remote-invoke grant is deleted locally so the
/// per-grant file policy does not survive as a ghost config. Fingerprint
/// policies are intentionally preserved because they can apply to future
/// grants created by the same SSH key or caller.
pub fn remove_grant_id_grant(grant_id: &str) -> Result<bool, String> {
    let mut cfg = load_raw_config();
    if !remove_grant_id_grant_in_place(&mut cfg, grant_id) {
        return Ok(false);
    }
    save_raw_config(&cfg)?;
    Ok(true)
}

pub(crate) fn remove_grant_id_grant_in_place(cfg: &mut RawConfig, grant_id: &str) -> bool {
    let before = cfg.grants.len();
    cfg.grants.retain(|g| {
        g.match_.grant_id.as_deref() != Some(grant_id) && g.grant_id.as_deref() != Some(grant_id)
    });
    cfg.grants.len() != before
}

/// Move an existing SSH-key policy from one fingerprint to another.
///
/// Used when an SSH key is rotated: the new key has a different fingerprint,
/// but the operator's configured roots/ops should follow the key unless no
/// explicit policy existed.
pub fn rekey_ssh_fingerprint_grant(
    old_fingerprint: &str,
    new_fingerprint: &str,
    name: Option<String>,
) -> Result<bool, String> {
    let mut cfg = load_raw_config();
    if !rekey_ssh_fingerprint_grant_in_place(&mut cfg, old_fingerprint, new_fingerprint, name) {
        return Ok(false);
    }
    save_raw_config(&cfg)?;
    Ok(true)
}

pub(crate) fn rekey_ssh_fingerprint_grant_in_place(
    cfg: &mut RawConfig,
    old_fingerprint: &str,
    new_fingerprint: &str,
    name: Option<String>,
) -> bool {
    let Some(index) = cfg
        .grants
        .iter()
        .position(|g| g.match_.ssh_fingerprint.as_deref() == Some(old_fingerprint))
    else {
        return false;
    };

    let mut moved = cfg.grants.remove(index);
    moved.match_.ssh_fingerprint = Some(new_fingerprint.to_string());
    if let Some(name) = name {
        moved.name = Some(name);
    }
    cfg.grants
        .retain(|g| g.match_.ssh_fingerprint.as_deref() != Some(new_fingerprint));
    cfg.grants.insert(0, moved);
    true
}

/// Pure in-place merge used by [`upsert_ssh_fingerprint_grant`]. Extracted
/// so tests can exercise the dedup/overwrite rules without touching the
/// on-disk TOML or the process-global data-dir singleton.
pub(crate) fn merge_ssh_fingerprint_grant_in_place(
    cfg: &mut RawConfig,
    fingerprint: &str,
    name: Option<String>,
    roots: Vec<PathBuf>,
    ops: Vec<FileOp>,
    allow_overwrite: Option<bool>,
    allow_recursive_delete: Option<bool>,
) {
    let ops_final = if ops.is_empty() { full_file_ops() } else { ops };
    let new_entry = RawGrantPolicy {
        match_: GrantMatch {
            grant_id: None,
            caller_fingerprint: None,
            ssh_fingerprint: Some(fingerprint.to_string()),
        },
        grant_id: None,
        name,
        roots,
        denies: Vec::new(),
        write_denies: Vec::new(),
        ops: ops_final,
        max_read_bytes: None,
        max_write_bytes: None,
        respect_gitignore: None,
        allow_overwrite,
        allow_recursive_delete,
    };
    for g in cfg.grants.iter_mut() {
        if g.match_.ssh_fingerprint.as_deref() == Some(fingerprint) {
            *g = new_entry;
            return;
        }
    }
    cfg.grants.insert(0, new_entry);
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

/// Public wrapper so sibling modules (e.g. `file_access_roots`) that surgically
/// rewrite `file-access.toml` without going through `save_raw_config` can still
/// signal the cache to re-parse on next `load_default()`.
pub(crate) fn invalidate_cache_pub() {
    invalidate_cache();
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
    const MAX_STORE_FILE_BYTES: u64 = 256 * 1024 * 1024;
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_STORE_FILE_BYTES {
        warn!(path = %path.display(), "file-access config too large, using default");
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

    #[test]
    fn cross_tier_priority_grant_id_beats_ssh_fp_beats_caller_fp() {
        // All three tiers match the same request. The resolver must
        // pick tier 1 (grant_id). If we then remove the grant_id
        // entry it must fall to tier 2 (ssh_fingerprint); removing
        // that must fall to tier 3 (caller_fingerprint).
        //
        // NOTE: the three entries are listed here in DESCENDING
        // specificity order; the test verifies precedence is driven
        // by tier, not by file order.
        let all_three = r#"
[[grant]]
match.caller_fingerprint = "caller-fp"
name = "via-caller-fp"
roots = ["/caller"]
ops = ["read"]

[[grant]]
match.ssh_fingerprint = "ssh-fp"
name = "via-ssh-fp"
roots = ["/ssh"]
ops = ["read", "list"]

[[grant]]
match.grant_id = "g-top"
name = "via-grant-id"
roots = ["/grant"]
ops = ["read", "list", "write"]
"#;
        let cwd = Path::new("/tmp");

        // Tier 1 wins.
        let p = parse(all_three).resolve("g-top", Some("caller-fp"), Some("ssh-fp"), cwd);
        assert_eq!(p.name, "via-grant-id");
        assert!(p.ops.contains(&FileOp::Write));

        // Drop tier-1 entry -> tier 2 (ssh_fp) wins.
        let without_grant_id = r#"
[[grant]]
match.caller_fingerprint = "caller-fp"
name = "via-caller-fp"
roots = ["/caller"]
ops = ["read"]

[[grant]]
match.ssh_fingerprint = "ssh-fp"
name = "via-ssh-fp"
roots = ["/ssh"]
ops = ["read", "list"]
"#;
        let p = parse(without_grant_id).resolve("g-top", Some("caller-fp"), Some("ssh-fp"), cwd);
        assert_eq!(p.name, "via-ssh-fp");
        assert!(!p.ops.contains(&FileOp::Write));

        // Drop tier-2 entry too -> tier 3 (caller_fp) wins.
        let caller_only = r#"
[[grant]]
match.caller_fingerprint = "caller-fp"
name = "via-caller-fp"
roots = ["/caller"]
ops = ["read"]
"#;
        let p = parse(caller_only).resolve("g-top", Some("caller-fp"), Some("ssh-fp"), cwd);
        assert_eq!(p.name, "via-caller-fp");
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

    #[test]
    fn full_file_ops_covers_all_twelve_ops() {
        let ops = full_file_ops();
        assert_eq!(ops.len(), 12);
        for expected in [
            FileOp::Read,
            FileOp::List,
            FileOp::Stat,
            FileOp::Glob,
            FileOp::Search,
            FileOp::Hash,
            FileOp::Write,
            FileOp::Edit,
            FileOp::Mkdir,
            FileOp::Move,
            FileOp::Delete,
            FileOp::ApplyPatch,
        ] {
            assert!(ops.contains(&expected), "missing op {:?}", expected);
        }
    }

    #[test]
    fn merge_ssh_fingerprint_grant_inserts_then_upserts_then_prepends() {
        let mut cfg = RawConfig::default();

        // 1) Fresh insert.
        merge_ssh_fingerprint_grant_in_place(
            &mut cfg,
            "fp-alpha",
            Some("ssh-key:laptop".to_string()),
            vec![PathBuf::from("/Users/tester")],
            full_file_ops(),
            Some(true),
            None,
        );
        assert_eq!(cfg.grants.len(), 1);
        assert_eq!(
            cfg.grants[0].match_.ssh_fingerprint.as_deref(),
            Some("fp-alpha")
        );
        assert_eq!(cfg.grants[0].ops.len(), 12);
        assert_eq!(cfg.grants[0].allow_overwrite, Some(true));

        // 2) Same fingerprint, different payload → overwrite, no duplicate.
        merge_ssh_fingerprint_grant_in_place(
            &mut cfg,
            "fp-alpha",
            Some("ssh-key:laptop-v2".to_string()),
            vec![PathBuf::from("/Users/tester/work")],
            vec![FileOp::Read, FileOp::List],
            None,
            None,
        );
        assert_eq!(
            cfg.grants.len(),
            1,
            "must not duplicate same-fingerprint entry"
        );
        assert_eq!(cfg.grants[0].ops, vec![FileOp::Read, FileOp::List]);
        assert_eq!(cfg.grants[0].name.as_deref(), Some("ssh-key:laptop-v2"));
        assert_eq!(
            cfg.grants[0].roots,
            vec![PathBuf::from("/Users/tester/work")]
        );

        // 3) Different fingerprint → prepended, old entry kept.
        merge_ssh_fingerprint_grant_in_place(
            &mut cfg,
            "fp-beta",
            None,
            vec![PathBuf::from("/opt/proj")],
            vec![], // empty → falls back to full_file_ops()
            None,
            None,
        );
        assert_eq!(cfg.grants.len(), 2);
        assert_eq!(
            cfg.grants[0].match_.ssh_fingerprint.as_deref(),
            Some("fp-beta")
        );
        assert_eq!(
            cfg.grants[0].ops.len(),
            12,
            "empty ops must default to full_file_ops"
        );
        assert_eq!(
            cfg.grants[1].match_.ssh_fingerprint.as_deref(),
            Some("fp-alpha")
        );

        // 4) Build a store from the merged config and check resolve hits the
        //    full-ops entry for fp-beta.
        let store = FileAccessPolicyStore::from_raw(cfg);
        let p = store.resolve("unknown-gid", None, Some("fp-beta"), Path::new("/tmp"));
        assert_eq!(p.ops.len(), 12);
        assert!(p.ops.contains(&FileOp::Write));
    }

    #[test]
    fn rekey_ssh_fingerprint_policy_preserves_roots_and_ops() {
        let mut cfg = RawConfig::default();
        merge_ssh_fingerprint_grant_in_place(
            &mut cfg,
            "old-fp",
            Some("ssh-key:old".to_string()),
            vec![PathBuf::from("/Users/tester/work")],
            vec![FileOp::Read, FileOp::List, FileOp::Write],
            Some(false),
            Some(false),
        );
        merge_ssh_fingerprint_grant_in_place(
            &mut cfg,
            "new-fp",
            Some("ssh-key:stale-new".to_string()),
            vec![PathBuf::from("/tmp/stale")],
            vec![FileOp::Read],
            None,
            None,
        );

        assert!(rekey_ssh_fingerprint_grant_in_place(
            &mut cfg,
            "old-fp",
            "new-fp",
            Some("ssh-key:new".to_string()),
        ));

        assert_eq!(cfg.grants.len(), 1);
        assert_eq!(
            cfg.grants[0].match_.ssh_fingerprint.as_deref(),
            Some("new-fp")
        );
        assert_eq!(cfg.grants[0].name.as_deref(), Some("ssh-key:new"));
        assert_eq!(
            cfg.grants[0].roots,
            vec![PathBuf::from("/Users/tester/work")]
        );
        assert_eq!(
            cfg.grants[0].ops,
            vec![FileOp::Read, FileOp::List, FileOp::Write]
        );
        assert_eq!(cfg.grants[0].allow_overwrite, Some(false));
    }

    #[test]
    fn rekey_ssh_fingerprint_policy_returns_false_when_missing() {
        let mut cfg = RawConfig::default();
        assert!(!rekey_ssh_fingerprint_grant_in_place(
            &mut cfg, "missing", "new-fp", None,
        ));
        assert!(cfg.grants.is_empty());
    }

    #[test]
    fn remove_grant_id_policy_removes_match_and_legacy_entries_only() {
        let mut cfg = RawConfig {
            grants: vec![
                RawGrantPolicy {
                    match_: GrantMatch {
                        grant_id: Some("grant-deleted".to_string()),
                        ..Default::default()
                    },
                    name: Some("match-grant".to_string()),
                    roots: vec![PathBuf::from("/grant")],
                    ops: vec![FileOp::Read],
                    ..Default::default()
                },
                RawGrantPolicy {
                    grant_id: Some("grant-deleted".to_string()),
                    name: Some("legacy-grant".to_string()),
                    roots: vec![PathBuf::from("/legacy")],
                    ops: vec![FileOp::Read],
                    ..Default::default()
                },
                RawGrantPolicy {
                    match_: GrantMatch {
                        ssh_fingerprint: Some("ssh-fp".to_string()),
                        ..Default::default()
                    },
                    name: Some("ssh-policy".to_string()),
                    roots: vec![PathBuf::from("/ssh")],
                    ops: vec![FileOp::Read],
                    ..Default::default()
                },
            ],
            default: Some(RawDefaultPolicy {
                roots: vec![PathBuf::from("/default")],
                ops: vec![FileOp::Read],
                ..Default::default()
            }),
        };

        assert!(remove_grant_id_grant_in_place(&mut cfg, "grant-deleted"));
        assert_eq!(cfg.grants.len(), 1);
        assert_eq!(cfg.grants[0].name.as_deref(), Some("ssh-policy"));
        assert!(cfg.default.is_some());
        assert!(!remove_grant_id_grant_in_place(&mut cfg, "grant-deleted"));
    }

    #[test]
    fn raw_config_has_ssh_fingerprint_grant_detects_present_policy() {
        let mut cfg = RawConfig::default();
        merge_ssh_fingerprint_grant_in_place(
            &mut cfg,
            "fp-present",
            None,
            vec![PathBuf::from("/tmp")],
            vec![FileOp::Read],
            None,
            None,
        );

        assert!(raw_config_has_ssh_fingerprint_grant(&cfg, "fp-present"));
        assert!(!raw_config_has_ssh_fingerprint_grant(&cfg, "fp-missing"));
    }
}
