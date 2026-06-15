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

/// The full set of file operations — Phase 1 read ops plus Phase 2 write ops
/// plus Phase 3 apply_patch. Serialized as snake_case strings on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOp {
    // --- Phase 1 (read) ---
    Read,
    ReadMany,
    List,
    Stat,
    Glob,
    Search,
    Hash,
    Outline,
    // --- Phase 2 (write) ---
    Write,
    Edit,
    Mkdir,
    Move,
    Delete,
    // --- Phase 3 (unified-diff apply) ---
    ApplyPatch,
}

impl FileOp {
    pub fn as_str(self) -> &'static str {
        match self {
            FileOp::Read => "read",
            FileOp::ReadMany => "read_many",
            FileOp::List => "list",
            FileOp::Stat => "stat",
            FileOp::Glob => "glob",
            FileOp::Search => "search",
            FileOp::Hash => "hash",
            FileOp::Outline => "outline",
            FileOp::Write => "write",
            FileOp::Edit => "edit",
            FileOp::Mkdir => "mkdir",
            FileOp::Move => "move",
            FileOp::Delete => "delete",
            FileOp::ApplyPatch => "apply_patch",
        }
    }

    /// True for any op that may mutate the filesystem.
    pub fn is_write(self) -> bool {
        matches!(
            self,
            FileOp::Write
                | FileOp::Edit
                | FileOp::Mkdir
                | FileOp::Move
                | FileOp::Delete
                | FileOp::ApplyPatch
        )
    }
}

/// The outcome of a successful policy check. The executor uses the
/// [`CanonicalPath`] for the actual syscall and the other fields for audit
/// logging.
#[derive(Debug, Clone)]
pub struct PolicyDecision {
    pub path: CanonicalPath,
    pub op: FileOp,
    /// Effective byte cap for the current read operation. For ops that don't
    /// read content this is informational.
    pub max_read_bytes: u64,
    /// Effective byte cap for the current write operation. Applies to
    /// `file.write`, `file.edit`, and per-file hunks of `file.apply_patch`.
    pub max_write_bytes: u64,
    pub respect_gitignore: bool,
    /// Whether write ops may overwrite an existing target. When false,
    /// `file.write` to an existing file is refused.
    pub allow_overwrite: bool,
    /// Whether `file.delete` may remove non-empty directories.
    pub allow_recursive_delete: bool,
    /// The absolute path as originally supplied by the caller, BEFORE
    /// canonicalization (i.e. without following symlinks). Handlers that
    /// need to detect/report symlinks (e.g. `file.stat`) should use this
    /// for `lstat`/`read_link`. Security decisions still use `path`
    /// (canonical, root-confined).
    pub input_abs: std::path::PathBuf,
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

    /// Additional deny patterns applied only to write ops. These layer on
    /// top of `denies` so read operations can continue to see files that
    /// Phase 2/3 must never mutate (example: `**/Cargo.lock`).
    #[serde(default)]
    pub write_denies: Vec<String>,

    /// The set of ops allowed by this policy. An empty set is invalid.
    pub ops: Vec<FileOp>,

    /// Maximum bytes that `file.read` will return. `file.search` also uses
    /// this to cap the per-file scan length.
    #[serde(default = "default_max_read_bytes")]
    pub max_read_bytes: u64,

    /// Maximum bytes accepted by a single write op. Defaults to 2 MiB to
    /// match `max_read_bytes` — large writes must be split.
    #[serde(default = "default_max_write_bytes")]
    pub max_write_bytes: u64,

    /// When true, paths ignored by `.gitignore` inside the matched root are
    /// treated as denied.
    #[serde(default = "default_true")]
    pub respect_gitignore: bool,

    /// Whether write ops may overwrite an existing file. Default: true.
    #[serde(default = "default_true")]
    pub allow_overwrite: bool,

    /// Whether `file.delete` may remove non-empty directories. Default: false.
    #[serde(default)]
    pub allow_recursive_delete: bool,
}

fn default_max_read_bytes() -> u64 {
    2 * 1024 * 1024
}
fn default_max_write_bytes() -> u64 {
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
    ///
    /// For write ops against a not-yet-existing target, we canonicalize the
    /// parent directory and rebuild the `CanonicalPath` via
    /// [`CanonicalPath::from_parent`] so containment checks still run
    /// against real, symlink-resolved paths.
    pub fn check(
        &self,
        input: &Path,
        cwd: &Path,
        op: FileOp,
    ) -> Result<PolicyDecision, FileAccessError> {
        // 0. remember the pre-canonicalization absolute input for handlers
        //    that need lstat semantics (e.g. `file.stat` symlink_target).
        let input_abs: std::path::PathBuf = if input.is_absolute() {
            input.to_path_buf()
        } else {
            cwd.join(input)
        };
        // 1. op allowlist
        if !self.ops.contains(&op) {
            if op.is_write() && self.ops.iter().all(|allowed| !allowed.is_write()) {
                return Err(FileAccessError::PermissionDenied {
                    reason: "readonly policy does not allow write operations",
                });
            }
            return Err(FileAccessError::OpNotPermitted { op: op.as_str() });
        }

        // 2. canonicalize + root containment + symlink escape detection.
        let canonical = match canonicalize_within_roots(input, cwd, &self.roots) {
            Ok(c) => c,
            Err(FileAccessError::NotFound { path }) if op.is_write() && op != FileOp::Delete => {
                let abs = if path.is_absolute() {
                    path.clone()
                } else {
                    cwd.join(&path)
                };
                let mut ancestor = abs.as_path();
                let mut missing_components = Vec::new();
                while !ancestor.exists() {
                    let component = ancestor
                        .file_name()
                        .ok_or_else(|| FileAccessError::OutOfScope { path: abs.clone() })?
                        .to_owned();
                    missing_components.push(component);
                    ancestor = ancestor
                        .parent()
                        .ok_or_else(|| FileAccessError::OutOfScope { path: abs.clone() })?;
                }

                let ancestor_canonical = canonicalize_within_roots(ancestor, cwd, &self.roots)?;
                let root_index = ancestor_canonical.root_index;
                let mut new_abs = ancestor_canonical.into_path_buf();
                for component in missing_components.iter().rev() {
                    new_abs.push(component);
                }
                let canonical_root = std::fs::canonicalize(&self.roots[root_index])
                    .unwrap_or_else(|_| self.roots[root_index].clone());
                let rel = new_abs
                    .strip_prefix(&canonical_root)
                    .map_err(|_| FileAccessError::OutOfScope {
                        path: new_abs.clone(),
                    })?
                    .to_string_lossy()
                    .replace('\\', "/");
                CanonicalPath::from_parent(new_abs, root_index, rel)
            }
            Err(e) => return Err(e),
        };

        // 3. deny-pattern check against the canonical, root-relative path
        let deny = DenyMatcher::new(&self.denies)?;
        if let Some(pat) = deny.match_raw(&canonical.rel_posix) {
            return Err(FileAccessError::DenyPattern {
                path: canonical.into_path_buf(),
                pattern: pat.to_string(),
            });
        }

        // 3b. write-specific deny layer
        if op.is_write() && !self.write_denies.is_empty() {
            let write_deny = DenyMatcher::new(&self.write_denies)?;
            if let Some(pat) = write_deny.match_raw(&canonical.rel_posix) {
                return Err(FileAccessError::DenyPattern {
                    path: canonical.into_path_buf(),
                    pattern: pat.to_string(),
                });
            }
        }

        // 4. gitignore (best-effort; implemented in Phase 1.1 when the
        //    `ignore` crate is wired in. For now we pass through.)

        Ok(PolicyDecision {
            path: canonical,
            op,
            max_read_bytes: self.max_read_bytes,
            max_write_bytes: self.max_write_bytes,
            respect_gitignore: self.respect_gitignore,
            allow_overwrite: self.allow_overwrite,
            allow_recursive_delete: self.allow_recursive_delete,
            input_abs,
        })
    }

    /// Convenience constructor used by tests + CLI dry-run.
    pub fn new_readonly(name: impl Into<String>, roots: Vec<PathBuf>) -> Self {
        Self {
            name: name.into(),
            roots,
            denies: default_denies(),
            write_denies: Vec::new(),
            ops: vec![
                FileOp::Read,
                FileOp::ReadMany,
                FileOp::List,
                FileOp::Stat,
                FileOp::Glob,
                FileOp::Search,
                FileOp::Hash,
                FileOp::Outline,
            ],
            max_read_bytes: default_max_read_bytes(),
            max_write_bytes: default_max_write_bytes(),
            respect_gitignore: true,
            allow_overwrite: true,
            allow_recursive_delete: false,
        }
    }

    /// Phase 2/3 convenience constructor: read + write + apply_patch.
    pub fn new_read_write(name: impl Into<String>, roots: Vec<PathBuf>) -> Self {
        Self {
            name: name.into(),
            roots,
            denies: default_denies(),
            write_denies: vec!["**/Cargo.lock".into(), "**/*.lock".into()],
            ops: vec![
                FileOp::Read,
                FileOp::ReadMany,
                FileOp::List,
                FileOp::Stat,
                FileOp::Glob,
                FileOp::Search,
                FileOp::Hash,
                FileOp::Outline,
                FileOp::Write,
                FileOp::Edit,
                FileOp::Mkdir,
                FileOp::Move,
                FileOp::Delete,
                FileOp::ApplyPatch,
            ],
            max_read_bytes: default_max_read_bytes(),
            max_write_bytes: default_max_write_bytes(),
            respect_gitignore: true,
            allow_overwrite: true,
            allow_recursive_delete: false,
        }
    }
}

fn default_denies() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // Sensitive directories: deny BOTH the directory itself (so `file.list`
    // can't enumerate its contents) AND everything inside it. A single
    // `**/dir/**` pattern does not match the bare `dir` path, which would
    // otherwise let callers list its entries.
    for dir in [".git", "target", "node_modules", ".ssh", ".aws", ".gnupg"] {
        out.push(format!("**/{dir}"));
        out.push(format!("**/{dir}/**"));
    }
    // macOS Keychains lives under Library/.
    out.push("**/Library/Keychains".into());
    out.push("**/Library/Keychains/**".into());
    // Sensitive file suffixes (match anywhere under roots).
    out.push("**/*.key".into());
    out.push("**/*.pem".into());
    out.push("**/*.pfx".into());
    out.push("**/*.p12".into());
    // dotenv and scoped env files.
    out.push("**/.env".into());
    out.push("**/.env.*".into());
    // Tool-specific credential files that commonly leak tokens.
    out.push("**/.npmrc".into());
    out.push("**/.netrc".into());
    // SSH private keys stored outside ~/.ssh (e.g. id_rsa copied
    // into a project dir). `id_rsa*` also matches id_rsa.pub /
    // id_rsa_old / id_rsa_backup etc.; operators who need access
    // can override via an explicit `[[grant]]` entry.
    out.push("**/id_rsa*".into());
    out
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
    fn readonly_policy_rejects_write_as_permission_denied() {
        let tmp = std::env::temp_dir();
        let policy = FileAccessPolicy::new_readonly("t", vec![tmp.clone()]);
        let err = policy
            .check(Path::new("Cargo.toml"), &tmp, FileOp::Write)
            .unwrap_err();
        assert_eq!(err.code(), "file.permission_denied");
    }

    #[test]
    fn default_policies_allow_read_many() {
        let root = std::env::temp_dir().join("bifrost_fa_read_many_test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("file.txt"), b"x").unwrap();

        let readonly = FileAccessPolicy::new_readonly("ro", vec![root.clone()]);
        let readwrite = FileAccessPolicy::new_read_write("rw", vec![root.clone()]);

        readonly
            .check(Path::new("file.txt"), &root, FileOp::ReadMany)
            .unwrap();
        readwrite
            .check(Path::new("file.txt"), &root, FileOp::ReadMany)
            .unwrap();

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_many_can_be_gated_independently_from_read() {
        let root = std::env::temp_dir().join("bifrost_fa_read_many_gate_test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("file.txt"), b"x").unwrap();

        let mut read_only = FileAccessPolicy::new_readonly("read-only", vec![root.clone()]);
        read_only.ops = vec![FileOp::Read];
        let err = read_only
            .check(Path::new("file.txt"), &root, FileOp::ReadMany)
            .unwrap_err();
        assert_eq!(err.code(), "file.op_not_permitted");
        read_only
            .check(Path::new("file.txt"), &root, FileOp::Read)
            .unwrap();

        let mut batch_only = FileAccessPolicy::new_readonly("batch-only", vec![root.clone()]);
        batch_only.ops = vec![FileOp::ReadMany];
        batch_only
            .check(Path::new("file.txt"), &root, FileOp::ReadMany)
            .unwrap();
        let err = batch_only
            .check(Path::new("file.txt"), &root, FileOp::Read)
            .unwrap_err();
        assert_eq!(err.code(), "file.op_not_permitted");

        std::fs::remove_dir_all(&root).ok();
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
        assert_eq!(err.code(), "file.deny_pattern");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn write_denies_layer_on_top_of_read_denies() {
        let tmp = std::env::temp_dir();
        let root = tmp.join("bifrost_fa_wdeny_test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Cargo.lock"), b"locked").unwrap();

        let policy = FileAccessPolicy::new_read_write("rw", vec![root.clone()]);
        // Read must succeed …
        policy
            .check(Path::new("Cargo.lock"), &root, FileOp::Read)
            .unwrap();
        // … Write must fail via write_denies.
        let err = policy
            .check(Path::new("Cargo.lock"), &root, FileOp::Write)
            .unwrap_err();
        assert_eq!(err.code(), "file.deny_pattern");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn write_on_missing_file_canonicalizes_parent() {
        let tmp = std::env::temp_dir();
        let root = tmp.join("bifrost_fa_missing_test");
        std::fs::create_dir_all(&root).unwrap();

        let policy = FileAccessPolicy::new_read_write("rw", vec![root.clone()]);
        // The target file does NOT exist — the check must still succeed by
        // canonicalizing the parent.
        let decision = policy
            .check(Path::new("new_file.txt"), &root, FileOp::Write)
            .unwrap();
        assert_eq!(decision.op, FileOp::Write);
        assert!(decision.path.as_path().ends_with("new_file.txt"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn mkdir_parents_can_target_nested_missing_path() {
        let tmp = std::env::temp_dir();
        let root = tmp.join("bifrost_fa_mkdir_parents_test");
        std::fs::create_dir_all(&root).unwrap();

        let policy = FileAccessPolicy::new_read_write("rw", vec![root.clone()]);
        let decision = policy
            .check(Path::new("sub/nested"), &root, FileOp::Mkdir)
            .unwrap();
        assert_eq!(decision.op, FileOp::Mkdir);
        assert!(decision.path.as_path().ends_with("sub/nested"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn default_denies_blocks_ssh_directory_listing() {
        let tmp = std::env::temp_dir();
        let root = tmp.join("bifrost_fa_ssh_dir_test");
        std::fs::create_dir_all(root.join(".ssh")).unwrap();
        std::fs::write(root.join(".ssh/id_rsa"), b"x").unwrap();

        let policy = FileAccessPolicy::new_readonly("t", vec![root.clone()]);

        // Listing the .ssh directory itself must be denied.
        let err = policy
            .check(Path::new(".ssh"), &root, FileOp::List)
            .unwrap_err();
        assert_eq!(err.code(), "file.deny_pattern");

        // Reading a file inside .ssh must also be denied.
        let err = policy
            .check(Path::new(".ssh/id_rsa"), &root, FileOp::Read)
            .unwrap_err();
        assert_eq!(err.code(), "file.deny_pattern");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn default_denies_blocks_target_and_node_modules_directory() {
        let tmp = std::env::temp_dir();
        let root = tmp.join("bifrost_fa_target_nm_test");
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        let policy = FileAccessPolicy::new_readonly("t", vec![root.clone()]);
        for d in ["target", "node_modules"] {
            let err = policy.check(Path::new(d), &root, FileOp::List).unwrap_err();
            assert_eq!(err.code(), "file.deny_pattern", "dir={d}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn default_denies_blocks_env_files() {
        let tmp = std::env::temp_dir();
        let root = tmp.join("bifrost_fa_env_test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".env"), b"x").unwrap();
        std::fs::write(root.join(".env.local"), b"x").unwrap();
        let policy = FileAccessPolicy::new_readonly("t", vec![root.clone()]);
        for f in [".env", ".env.local"] {
            let err = policy.check(Path::new(f), &root, FileOp::Read).unwrap_err();
            assert_eq!(err.code(), "file.deny_pattern", "file={f}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn default_denies_blocks_tool_credential_files() {
        // .npmrc / .netrc commonly contain auth tokens.
        let tmp = std::env::temp_dir();
        let root = tmp.join("bifrost_fa_tool_cred_test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".npmrc"), b"//registry.npmjs.org/:_authToken=xxx").unwrap();
        std::fs::write(
            root.join(".netrc"),
            b"machine example.com login u password p",
        )
        .unwrap();
        let policy = FileAccessPolicy::new_readonly("t", vec![root.clone()]);
        for f in [".npmrc", ".netrc"] {
            let err = policy.check(Path::new(f), &root, FileOp::Read).unwrap_err();
            assert_eq!(err.code(), "file.deny_pattern", "file={f}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn default_denies_blocks_ssh_private_key_files() {
        // id_rsa* anywhere under a root (not just inside ~/.ssh).
        let tmp = std::env::temp_dir();
        let root = tmp.join("bifrost_fa_idrsa_test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("id_rsa"), b"-----BEGIN-----").unwrap();
        std::fs::write(root.join("id_rsa.pub"), b"ssh-rsa ...").unwrap();
        std::fs::write(root.join("id_rsa_backup"), b"-----BEGIN-----").unwrap();
        let policy = FileAccessPolicy::new_readonly("t", vec![root.clone()]);
        for f in ["id_rsa", "id_rsa.pub", "id_rsa_backup"] {
            let err = policy.check(Path::new(f), &root, FileOp::Read).unwrap_err();
            assert_eq!(err.code(), "file.deny_pattern", "file={f}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn default_denies_blocks_pfx_and_p12_bundles() {
        // PKCS#12 / PFX bundles carry private keys.
        let tmp = std::env::temp_dir();
        let root = tmp.join("bifrost_fa_pfx_test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("client.pfx"), b"\x30\x82").unwrap();
        std::fs::write(root.join("client.p12"), b"\x30\x82").unwrap();
        let policy = FileAccessPolicy::new_readonly("t", vec![root.clone()]);
        for f in ["client.pfx", "client.p12"] {
            let err = policy.check(Path::new(f), &root, FileOp::Read).unwrap_err();
            assert_eq!(err.code(), "file.deny_pattern", "file={f}");
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
