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

use bifrost_core::file_access::{FileAccessPolicy, FileOp};
use serde::Deserialize;
use tracing::{debug, warn};

const CONFIG_FILE_NAME: &str = "file-access.toml";

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default, rename = "grant")]
    grants: Vec<RawGrantPolicy>,
}

#[derive(Debug, Deserialize)]
struct RawGrantPolicy {
    grant_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    roots: Vec<PathBuf>,
    #[serde(default)]
    denies: Vec<String>,
    #[serde(default)]
    write_denies: Vec<String>,
    #[serde(default)]
    ops: Vec<FileOp>,
    #[serde(default)]
    max_read_bytes: Option<u64>,
    #[serde(default)]
    max_write_bytes: Option<u64>,
    #[serde(default)]
    respect_gitignore: Option<bool>,
    #[serde(default)]
    allow_overwrite: Option<bool>,
    #[serde(default)]
    allow_recursive_delete: Option<bool>,
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

    /// Load from `<data-dir>/file-access.toml` if it exists. Missing file or
    /// parse errors produce an empty store plus a warning — the relay can
    /// still serve requests using the default read-only policy.
    pub fn load_default() -> Self {
        let path = default_config_path();
        if path.exists() {
            Self::load_from(&path)
        } else {
            Self::empty()
        }
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

fn default_config_path() -> PathBuf {
    bifrost_storage::data_dir().join(CONFIG_FILE_NAME)
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
}
