//! Fine-grained CRUD over `[[grant]].roots` in `file-access.toml`.
//!
//! This module powers the Web UI's per-grant path picker. It does NOT rewrite
//! whole `RawConfig` blobs (that would clobber user comments / custom fields).
//! Instead it uses `toml_edit` to surgically update the `roots` array of the
//! `[[grant]]` entry whose `match.grant_id` matches, preserving everything
//! else byte-for-byte.
//!
//! Semantics (see design doc `docs/design/file-access-ui-path-picker.md`):
//! - Paths are canonicalized with `std::fs::canonicalize` before insert.
//! - Parent/child merging: inserting `/a/b` when `/a` already exists is a
//!   no-op; inserting `/a` when `/a/b` exists replaces `/a/b` with `/a`.
//! - Duplicates (same canonical path) are silently dropped.
//! - Missing directories or non-directory paths are rejected with
//!   `RootEditError::InvalidPath`.
//! - `[default]` is NEVER touched by this module.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use toml_edit::{Array, DocumentMut, Item, Value};
use tracing::{debug, warn};

use super::file_policy_store::invalidate_cache_pub;

const CONFIG_FILE_NAME: &str = "file-access.toml";

/// Coarse in-process lock around TOML read-modify-write. The config is small
/// and only ever written from HTTP handlers, so this is cheap and avoids
/// partial rewrites across concurrent UI tabs.
static WRITE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Debug, thiserror::Error)]
pub enum RootEditError {
    #[error("grant '{0}' not found in file-access.toml")]
    GrantNotFound(String),
    #[error("path does not exist or is not a directory: {0}")]
    InvalidPath(PathBuf),
    #[error("failed to canonicalize path: {0}")]
    Canonicalize(String),
    #[error("failed to read config: {0}")]
    Io(String),
    #[error("failed to parse TOML: {0}")]
    Parse(String),
    #[error("failed to write config: {0}")]
    Write(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatePathResult {
    pub exists: bool,
    pub is_dir: bool,
    pub canonical: Option<PathBuf>,
    pub reason: Option<String>,
}

/// Outcome of a successful `add_root`. `replaced_children` lists any prior
/// root entries that were subsumed by the new parent and removed.
#[derive(Debug, Clone, Serialize)]
pub struct AddRootOutcome {
    pub grant_id: String,
    pub canonical: PathBuf,
    pub already_present: bool,
    pub replaced_children: Vec<PathBuf>,
}

fn config_path() -> PathBuf {
    bifrost_storage::data_dir().join(CONFIG_FILE_NAME)
}

pub fn validate_path(input: &Path) -> ValidatePathResult {
    match std::fs::metadata(input) {
        Ok(meta) if meta.is_dir() => {
            let canonical = std::fs::canonicalize(input).ok();
            ValidatePathResult {
                exists: true,
                is_dir: true,
                canonical,
                reason: None,
            }
        }
        Ok(_) => ValidatePathResult {
            exists: true,
            is_dir: false,
            canonical: None,
            reason: Some("not a directory".into()),
        },
        Err(e) => ValidatePathResult {
            exists: false,
            is_dir: false,
            canonical: None,
            reason: Some(e.to_string()),
        },
    }
}

fn canonicalize_dir(input: &Path) -> Result<PathBuf, RootEditError> {
    let meta =
        std::fs::metadata(input).map_err(|_| RootEditError::InvalidPath(input.to_path_buf()))?;
    if !meta.is_dir() {
        return Err(RootEditError::InvalidPath(input.to_path_buf()));
    }
    std::fs::canonicalize(input).map_err(|e| RootEditError::Canonicalize(e.to_string()))
}

fn is_subpath(child: &Path, parent: &Path) -> bool {
    child.starts_with(parent) && child != parent
}

fn load_doc_at(path: &Path) -> Result<DocumentMut, RootEditError> {
    const MAX_STORE_FILE_BYTES: u64 = 256 * 1024 * 1024;
    let raw = if path.exists() {
        if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > MAX_STORE_FILE_BYTES {
            return Err(RootEditError::Io(format!(
                "file too large: {}",
                path.display()
            )));
        }
        std::fs::read_to_string(path).map_err(|e| RootEditError::Io(e.to_string()))?
    } else {
        String::new()
    };
    raw.parse::<DocumentMut>()
        .map_err(|e| RootEditError::Parse(e.to_string()))
}

fn save_doc_at(path: &Path, doc: &DocumentMut) -> Result<(), RootEditError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| RootEditError::Write(format!("mkdir parent: {e}")))?;
    }
    // Atomic write: tmp + rename.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())
        .map_err(|e| RootEditError::Write(format!("write tmp: {e}")))?;
    std::fs::rename(&tmp, path).map_err(|e| RootEditError::Write(format!("rename: {e}")))?;
    debug!(path = %path.display(), "saved file-access.toml (roots update)");
    Ok(())
}

/// Locate the `[[grant]]` array-of-tables entry whose `match.grant_id` or
/// legacy flat `grant_id` equals `grant_id`. Returns the positional index.
fn find_grant_index(doc: &DocumentMut, grant_id: &str) -> Option<usize> {
    let arr = doc.get("grant")?.as_array_of_tables()?;
    for (idx, tbl) in arr.iter().enumerate() {
        if let Some(m) = tbl.get("match").and_then(Item::as_table_like) {
            if let Some(v) = m.get("grant_id").and_then(Item::as_str) {
                if v == grant_id {
                    return Some(idx);
                }
            }
        }
        if let Some(v) = tbl.get("grant_id").and_then(Item::as_str) {
            if v == grant_id {
                return Some(idx);
            }
        }
    }
    None
}

fn roots_array_mut(doc: &mut DocumentMut, idx: usize) -> Option<&mut Array> {
    let arr = doc.get_mut("grant")?.as_array_of_tables_mut()?;
    let tbl = arr.get_mut(idx)?;
    if !tbl.contains_key("roots") {
        tbl.insert("roots", Item::Value(Value::Array(Array::new())));
    }
    tbl.get_mut("roots")?.as_array_mut()
}

pub fn add_root(grant_id: &str, input: &Path) -> Result<AddRootOutcome, RootEditError> {
    let outcome = add_root_at(&config_path(), grant_id, input)?;
    invalidate_cache_pub();
    Ok(outcome)
}

/// Same as [`add_root`] but writes to an explicit config path, and does NOT
/// touch the `FileAccessPolicyStore` cache. Useful for tests that want to
/// avoid the process-wide `data_dir` global.
pub fn add_root_at(
    config: &Path,
    grant_id: &str,
    input: &Path,
) -> Result<AddRootOutcome, RootEditError> {
    let _guard = WRITE_LOCK.lock().expect("WRITE_LOCK poisoned");
    let canonical = canonicalize_dir(input)?;

    let mut doc = load_doc_at(config)?;
    let idx = find_grant_index(&doc, grant_id)
        .ok_or_else(|| RootEditError::GrantNotFound(grant_id.to_string()))?;

    let arr = roots_array_mut(&mut doc, idx)
        .ok_or_else(|| RootEditError::Parse("grant.roots not an array".into()))?;

    // Collect current roots as PathBufs.
    let mut current: Vec<PathBuf> = arr
        .iter()
        .filter_map(|v| v.as_str().map(PathBuf::from))
        .collect();

    // Already present?
    if current.iter().any(|p| p == &canonical) {
        return Ok(AddRootOutcome {
            grant_id: grant_id.to_string(),
            canonical,
            already_present: true,
            replaced_children: vec![],
        });
    }

    // Is the new path inside an existing root? No-op, but not "already_present".
    if current.iter().any(|p| is_subpath(&canonical, p)) {
        return Ok(AddRootOutcome {
            grant_id: grant_id.to_string(),
            canonical,
            already_present: true,
            replaced_children: vec![],
        });
    }

    // New path is a parent of some existing roots: remove those children.
    let (children, rest): (Vec<_>, Vec<_>) =
        current.drain(..).partition(|p| is_subpath(p, &canonical));

    // Rebuild array: keep `rest`, append `canonical`.
    arr.clear();
    for p in &rest {
        arr.push(p.to_string_lossy().to_string());
    }
    arr.push(canonical.to_string_lossy().to_string());

    save_doc_at(config, &doc)?;
    Ok(AddRootOutcome {
        grant_id: grant_id.to_string(),
        canonical,
        already_present: false,
        replaced_children: children,
    })
}

pub fn remove_root(grant_id: &str, target: &Path) -> Result<bool, RootEditError> {
    let removed = remove_root_at(&config_path(), grant_id, target)?;
    if removed {
        invalidate_cache_pub();
    }
    Ok(removed)
}

pub fn remove_root_at(config: &Path, grant_id: &str, target: &Path) -> Result<bool, RootEditError> {
    let _guard = WRITE_LOCK.lock().expect("WRITE_LOCK poisoned");
    let mut doc = load_doc_at(config)?;
    let idx = find_grant_index(&doc, grant_id)
        .ok_or_else(|| RootEditError::GrantNotFound(grant_id.to_string()))?;

    let arr = roots_array_mut(&mut doc, idx)
        .ok_or_else(|| RootEditError::Parse("grant.roots not an array".into()))?;

    // Accept either literal stored string OR canonicalized form.
    let canonical = std::fs::canonicalize(target).ok();
    let target_str = target.to_string_lossy().to_string();
    let canonical_str = canonical.as_ref().map(|p| p.to_string_lossy().to_string());

    let mut removed = false;
    let kept: Vec<String> = arr
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .filter(|s| {
            let hit = s == &target_str || (canonical_str.as_ref() == Some(s));
            if hit {
                removed = true;
                false
            } else {
                true
            }
        })
        .collect();

    if !removed {
        warn!(grant_id, path = ?target, "remove_root: path not found in grant.roots");
        return Ok(false);
    }

    arr.clear();
    for s in kept {
        arr.push(s);
    }
    save_doc_at(config, &doc)?;
    Ok(true)
}

pub fn list_roots(grant_id: &str) -> Result<Vec<String>, RootEditError> {
    list_roots_at(&config_path(), grant_id)
}

/// Read-only variant that does not invalidate the policy cache. Returns the
/// roots currently stored for `grant_id` (raw strings, unmodified).
pub fn list_roots_at(config: &Path, grant_id: &str) -> Result<Vec<String>, RootEditError> {
    let doc = load_doc_at(config)?;
    let idx = find_grant_index(&doc, grant_id)
        .ok_or_else(|| RootEditError::GrantNotFound(grant_id.to_string()))?;
    let arr = doc
        .get("grant")
        .and_then(Item::as_array_of_tables)
        .and_then(|a| a.get(idx))
        .and_then(|t| t.get("roots"))
        .and_then(Item::as_array);
    Ok(arr
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Drop-in helper: create a fresh tempdir with a `file-access.toml` path,
    /// hand it to the test, and clean up. Does NOT touch the process-wide
    /// `data_dir` global, so these tests are safe to run in parallel with the
    /// rest of the crate.
    fn with_tempdir<F: FnOnce(&Path, &Path)>(f: F) {
        let td = TempDir::new().unwrap();
        let cfg = td.path().join(CONFIG_FILE_NAME);
        f(td.path(), &cfg);
    }

    fn seed(cfg: &Path, body: &str) {
        std::fs::write(cfg, body).unwrap();
    }

    #[test]
    fn add_root_basic() {
        with_tempdir(|_dir, cfg| {
            let td = TempDir::new().unwrap();
            let sub = td.path().join("repo");
            std::fs::create_dir(&sub).unwrap();
            seed(
                cfg,
                r#"[[grant]]
match.grant_id = "g1"
roots = []
ops = ["read"]
"#,
            );
            let out = add_root_at(cfg, "g1", &sub).unwrap();
            assert!(!out.already_present);
            let body = std::fs::read_to_string(cfg).unwrap();
            assert!(body.contains("repo"));
        });
    }

    #[test]
    fn add_parent_replaces_child() {
        with_tempdir(|_dir, cfg| {
            let td = TempDir::new().unwrap();
            let parent = td.path().join("proj");
            let child = parent.join("src");
            std::fs::create_dir_all(&child).unwrap();
            let parent_c = std::fs::canonicalize(&parent).unwrap();
            let child_c = std::fs::canonicalize(&child).unwrap();
            seed(
                cfg,
                &format!(
                    "[[grant]]\nmatch.grant_id = \"g1\"\nroots = [\"{}\"]\nops = [\"read\"]\n",
                    child_c.display()
                ),
            );
            let out = add_root_at(cfg, "g1", &parent).unwrap();
            assert_eq!(out.canonical, parent_c);
            assert_eq!(out.replaced_children.len(), 1);
            let body = std::fs::read_to_string(cfg).unwrap();
            assert!(!body.contains(&child_c.to_string_lossy().to_string()));
            assert!(body.contains(&parent_c.to_string_lossy().to_string()));
        });
    }

    #[test]
    fn add_child_noop_when_parent_present() {
        with_tempdir(|_dir, cfg| {
            let td = TempDir::new().unwrap();
            let parent = td.path().join("proj");
            let child = parent.join("src");
            std::fs::create_dir_all(&child).unwrap();
            let parent_c = std::fs::canonicalize(&parent).unwrap();
            seed(
                cfg,
                &format!(
                    "[[grant]]\nmatch.grant_id = \"g1\"\nroots = [\"{}\"]\nops = [\"read\"]\n",
                    parent_c.display()
                ),
            );
            let out = add_root_at(cfg, "g1", &child).unwrap();
            assert!(out.already_present);
        });
    }

    #[test]
    fn reject_missing_dir() {
        with_tempdir(|_dir, cfg| {
            seed(
                cfg,
                "[[grant]]\nmatch.grant_id = \"g1\"\nroots = []\nops = [\"read\"]\n",
            );
            let bogus = Path::new("/definitely/does/not/exist/xyz-bifrost-test");
            assert!(matches!(
                add_root_at(cfg, "g1", bogus),
                Err(RootEditError::InvalidPath(_))
            ));
        });
    }

    #[test]
    fn grant_not_found() {
        with_tempdir(|_dir, cfg| {
            let td = TempDir::new().unwrap();
            let real = td.path();
            seed(
                cfg,
                "[[grant]]\nmatch.grant_id = \"g1\"\nroots = []\nops = [\"read\"]\n",
            );
            assert!(matches!(
                add_root_at(cfg, "g-missing", real),
                Err(RootEditError::GrantNotFound(_))
            ));
        });
    }

    #[test]
    fn preserves_other_grant_entries_and_comments() {
        with_tempdir(|_dir, cfg| {
            let td = TempDir::new().unwrap();
            let sub = td.path().join("repo");
            std::fs::create_dir(&sub).unwrap();
            let body = r#"# top comment
[[grant]]
match.grant_id = "g1"
name = "alpha"
roots = []
ops = ["read"]

[[grant]]
match.grant_id = "g2"
roots = ["/tmp/other"]
ops = ["read"]

[default]
roots = ["/tmp"]
ops = ["read"]
"#;
            seed(cfg, body);
            add_root_at(cfg, "g1", &sub).unwrap();
            let after = std::fs::read_to_string(cfg).unwrap();
            assert!(after.contains("# top comment"));
            assert!(after.contains(r#""/tmp/other""#));
            assert!(after.contains("[default]"));
        });
    }

    #[test]
    fn remove_root_works() {
        with_tempdir(|_dir, cfg| {
            seed(
                cfg,
                r#"[[grant]]
match.grant_id = "g1"
roots = ["/tmp/a", "/tmp/b"]
ops = ["read"]
"#,
            );
            let ok = remove_root_at(cfg, "g1", Path::new("/tmp/a")).unwrap();
            assert!(ok);
            let after = std::fs::read_to_string(cfg).unwrap();
            assert!(!after.contains(r#""/tmp/a""#));
            assert!(after.contains(r#""/tmp/b""#));
        });
    }

    #[test]
    fn list_roots_returns_stored_values() {
        with_tempdir(|_dir, cfg| {
            let td = TempDir::new().unwrap();
            let a = td.path().join("a");
            let b = td.path().join("b");
            std::fs::create_dir(&a).unwrap();
            std::fs::create_dir(&b).unwrap();
            let ac = std::fs::canonicalize(&a).unwrap();
            let bc = std::fs::canonicalize(&b).unwrap();
            seed(
                cfg,
                &format!(
                    "[[grant]]\nmatch.grant_id = \"g1\"\nroots = [\"{}\", \"{}\"]\nops = [\"read\"]\n",
                    ac.display(),
                    bc.display()
                ),
            );
            let listed = list_roots_at(cfg, "g1").unwrap();
            assert_eq!(listed.len(), 2);
            assert!(listed
                .iter()
                .any(|s| s == &ac.to_string_lossy().to_string()));
            assert!(listed
                .iter()
                .any(|s| s == &bc.to_string_lossy().to_string()));
        });
    }

    #[test]
    fn list_roots_returns_empty_when_key_missing() {
        with_tempdir(|_dir, cfg| {
            seed(
                cfg,
                "[[grant]]\nmatch.grant_id = \"g1\"\nops = [\"read\"]\n",
            );
            let listed = list_roots_at(cfg, "g1").unwrap();
            assert!(listed.is_empty());
        });
    }

    #[test]
    fn list_roots_errors_on_missing_grant() {
        with_tempdir(|_dir, cfg| {
            seed(
                cfg,
                "[[grant]]\nmatch.grant_id = \"g1\"\nroots = []\nops = [\"read\"]\n",
            );
            assert!(matches!(
                list_roots_at(cfg, "g-missing"),
                Err(RootEditError::GrantNotFound(_))
            ));
        });
    }

    #[test]
    fn duplicate_add_is_idempotent() {
        with_tempdir(|_dir, cfg| {
            let td = TempDir::new().unwrap();
            let sub = td.path().join("repo");
            std::fs::create_dir(&sub).unwrap();
            let canonical = std::fs::canonicalize(&sub).unwrap();
            seed(
                cfg,
                &format!(
                    "[[grant]]\nmatch.grant_id = \"g1\"\nroots = [\"{}\"]\nops = [\"read\"]\n",
                    canonical.display()
                ),
            );
            let out = add_root_at(cfg, "g1", &sub).unwrap();
            assert!(out.already_present);
            assert_eq!(out.replaced_children.len(), 0);
            // File should remain byte-stable: one entry, not duplicated.
            let after = std::fs::read_to_string(cfg).unwrap();
            let count = after
                .matches(&canonical.to_string_lossy().to_string())
                .count();
            assert_eq!(count, 1);
        });
    }

    #[test]
    fn remove_missing_returns_false_without_error() {
        with_tempdir(|_dir, cfg| {
            seed(
                cfg,
                r#"[[grant]]
match.grant_id = "g1"
roots = ["/tmp/a"]
ops = ["read"]
"#,
            );
            let removed = remove_root_at(cfg, "g1", Path::new("/tmp/nonexistent")).unwrap();
            assert!(!removed);
        });
    }

    #[test]
    fn legacy_flat_grant_id_matching_works() {
        // Older configs may have used flat `grant_id = "..."` instead of
        // `match.grant_id = "..."`. We support both.
        with_tempdir(|_dir, cfg| {
            let td = TempDir::new().unwrap();
            let sub = td.path().join("repo");
            std::fs::create_dir(&sub).unwrap();
            seed(
                cfg,
                r#"[[grant]]
grant_id = "legacy"
roots = []
ops = ["read"]
"#,
            );
            let out = add_root_at(cfg, "legacy", &sub).unwrap();
            assert!(!out.already_present);
        });
    }

    #[test]
    fn validate_path_on_file_returns_not_dir() {
        let td = TempDir::new().unwrap();
        let file = td.path().join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        let r = validate_path(&file);
        assert!(r.exists);
        assert!(!r.is_dir);
    }

    #[test]
    fn validate_path_on_missing_returns_not_exists() {
        let r = validate_path(Path::new("/definitely/not/here/xyz-bifrost"));
        assert!(!r.exists);
        assert!(!r.is_dir);
        assert!(r.reason.is_some());
    }

    #[test]
    fn validate_path_on_dir_returns_canonical() {
        let td = TempDir::new().unwrap();
        let r = validate_path(td.path());
        assert!(r.exists);
        assert!(r.is_dir);
        assert!(r.canonical.is_some());
    }
}
