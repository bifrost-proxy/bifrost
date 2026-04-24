//! `FileAccessPolicy` — declarative, per-grant rules that govern every
//! `file.*` remote invoke call.
//!
//! A policy is a pure value (no IO on construction), loaded from the admin
//! config. The executor calls [`FileAccessPolicy::check`] exactly once per
//! request before dispatching to the filesystem.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::file_access::{
    error::FileAccessError,
    matcher::DenyMatcher,
    path::{canonicalize_within_roots, CanonicalPath},
};

/// The set of Phase 1 file operations. This enum is serialized as lowercase
/// kebab strings in the config and on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileOp {
    Read,
    List,
    Stat,
    Glob,
    Search,
    Hash,
}

impl FileOp {
    pub fn as_str(self) -> &'static str {
        match self {
            FileOp::Read => "read",
            FileOp::List => "list",
            FileOp::Stat => "stat",
            FileOp::Glob => "glob",
            FileOp::Search => "search",
            FileOp::Hash => "hash",
        }
    }
}

/// The outcome of a successful policy check. The executor uses the
/// [`CanonicalPath`] for the actual syscall and the other fields for audit
/// logging.
#[derive(Debug, Clone)]
pub struct PolicyDecision {
    pub path: CanonicalPath,
    pub op: FileOp,
    /// Effective byte cap for the current operation. For ops that don't
    /// read content (e.g. `stat`, `hash`) this is informational.
    pub max_read_bytes: u64,
    pub respect_gitignore: bool,
}

/// Declarative file access policy. One policy corresponds to one grant +
/// scope pair; the admin config can hold many policies and resolve by grant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAccessPolicy {
    /// Human-readable label, surfaced in audit logs + CLI listings.
    pub name: String,

    /// The allowlisted root directories. A request path must canonicalize to
    /// somewhere inside at least one root.
    pub roots: Vec<PathBuf>,

    /// Glob patterns (root-relative) that are always denied, even if
    /// `roots` would otherwise allow them. Example: `**/.git/**`.
    #[serde(default)]
    pub denies: Vec<String>,

    /// The set of ops allowed by this policy. An empty set is invalid.
    pub ops: Vec<FileOp>,

    /// Maximum bytes that `file.read` will return. `file.search` also uses
    /// this to cap the per-file scan length.
    #[serde(default = "default_max_read_bytes")]
    pub max_read_bytes: u64,

    /// When true, paths ignored by `.gitignore` inside the matched root are
    /// treated as denied.
    #[serde(default = "default_true")]
    pub respect_gitignore: bool,
}

fn default_max_read_bytes() -> u64 {
    2 * 1024 * 1024
}
fn default_true() -> bool {
    true
}

impl FileAccessPolicy {
    /// Evaluate the policy for a given (input path, cwd, op).
    ///
    /// This function performs *all* security checks required before any
    /// filesystem read. Callers MUST NOT short-circuit or skip it.
    pub fn check(
        &self,
        input: &Path,
        cwd: &Path,
        op: FileOp,
    ) -> Result<PolicyDecision, FileAccessError> {
        // 1. op allowlist
        if !self.ops.contains(&op) {
            return Err(FileAccessError::OpNotPermitted { op: op.as_str() });
        }

        // 2. canonicalize + root containment + symlink escape detection
        let canonical = canonicalize_within_roots(input, cwd, &self.roots)?;

        // 3. deny-pattern check against the canonical, root-relative path
        let deny = DenyMatcher::new(&self.denies)?;
        if let Some(pat) = deny.match_raw(&canonical.rel_posix) {
            return Err(FileAccessError::DenyPattern {
                path: canonical.into_path_buf(),
                pattern: pat.to_string(),
            });
        }

        // 4. gitignore (best-effort; implemented in Phase 1.1 when the
        //    `ignore` crate is wired in. For now we pass through.)
        //
        // TODO(phase-1.1): consult `ignore::gitignore::Gitignore` rooted at
        // `self.roots[canonical.root_index]` and fail with
        // `FileAccessError::IgnoredByGitignore` on a match.

        Ok(PolicyDecision {
            path: canonical,
            op,
            max_read_bytes: self.max_read_bytes,
            respect_gitignore: self.respect_gitignore,
        })
    }

    /// Convenience constructor used by tests + CLI dry-run.
    pub fn new_readonly(name: impl Into<String>, roots: Vec<PathBuf>) -> Self {
        Self {
            name: name.into(),
            roots,
            denies: vec![
                "**/.git/**".into(),
                "**/target/**".into(),
                "**/*.key".into(),
                "**/*.pem".into(),
            ],
            ops: vec![
                FileOp::Read,
                FileOp::List,
                FileOp::Stat,
                FileOp::Glob,
                FileOp::Search,
                FileOp::Hash,
            ],
            max_read_bytes: default_max_read_bytes(),
            respect_gitignore: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_not_in_allowlist_rejected() {
        let tmp = std::env::temp_dir();
        let mut policy = FileAccessPolicy::new_readonly("t", vec![tmp.clone()]);
        policy.ops = vec![FileOp::Stat]; // no Read
        let err = policy
            .check(Path::new("Cargo.toml"), &tmp, FileOp::Read)
            .unwrap_err();
        assert_eq!(err.code(), "file.op_not_permitted");
    }

    #[test]
    fn deny_pattern_fires() {
        let tmp = std::env::temp_dir();
        let root = tmp.join("bifrost_fa_deny_test");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), b"x").unwrap();

        let policy = FileAccessPolicy::new_readonly("t", vec![root.clone()]);
        let err = policy
            .check(Path::new(".git/config"), &root, FileOp::Read)
            .unwrap_err();
        assert_eq!(err.code(), "file.permission_denied");

        std::fs::remove_dir_all(&root).ok();
    }
}
