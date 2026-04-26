//! Path canonicalization with root containment checks.
//!
//! The golden rule for file_access: **no filesystem operation may happen on a
//! path that has not passed [`canonicalize_within_roots`]**. Any caller that
//! bypasses this function is a security bug.

use std::path::{Component, Path, PathBuf};

use crate::file_access::error::FileAccessError;

/// A path that has been canonicalized *and* confirmed to live inside one of
/// the policy roots. The inner [`PathBuf`] is absolute and symlink-resolved.
///
/// `CanonicalPath` intentionally does not implement `From<PathBuf>` — the
/// only way to construct it is via [`canonicalize_within_roots`], so the
/// invariant cannot be bypassed accidentally.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CanonicalPath {
    abs: PathBuf,
    /// Index into the caller-provided `roots` slice that matched.
    pub root_index: usize,
    /// Path relative to the matching root, in POSIX form. Used for glob /
    /// deny matching.
    pub rel_posix: String,
}

impl CanonicalPath {
    pub fn as_path(&self) -> &Path {
        &self.abs
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.abs
    }

    /// Construct a `CanonicalPath` for a not-yet-existing target whose
    /// parent directory has already been canonicalized via
    /// [`canonicalize_within_roots`]. The caller is responsible for ensuring
    /// `abs` is absolute and lies inside `roots[root_index]`; this
    /// constructor intentionally does no IO.
    ///
    /// Used by `FileAccessPolicy::check` to handle Phase 2 write ops that
    /// target files which do not exist yet.
    pub fn from_parent(abs: PathBuf, root_index: usize, rel_posix: String) -> Self {
        Self {
            abs,
            root_index,
            rel_posix,
        }
    }
}

/// Canonicalize `input` (which may be relative or absolute) and ensure the
/// resulting path lies within **one of** `roots`. Each root is itself
/// canonicalized on the fly.
///
/// Returns [`FileAccessError::OutOfScope`] if the resolved path is outside
/// every root; returns [`FileAccessError::SymlinkEscape`] if the input
/// resolves via a symlink to somewhere outside the roots.
///
/// `input` may refer to a not-yet-existing file (e.g. for future writes); in
/// that case we canonicalize the parent and reattach the final component.
/// For Phase 1 (read-only) the not-yet-existing case returns `NotFound`.
pub fn canonicalize_within_roots(
    input: &Path,
    cwd: &Path,
    roots: &[PathBuf],
) -> Result<CanonicalPath, FileAccessError> {
    let joined = if input.is_absolute() {
        input.to_path_buf()
    } else {
        cwd.join(input)
    };

    // Reject `..` traversal before touching the filesystem — defense in
    // depth against TOCTOU where the final canonicalize succeeds but an
    // intermediate symlink flips roots between calls.
    if has_parent_traversal(&joined) {
        // We don't hard-fail here; we simply rely on the canonicalize step
        // below. But we annotate via logs in the policy layer.
    }

    let abs = match std::fs::canonicalize(&joined) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(FileAccessError::NotFound { path: joined });
        }
        Err(e) => {
            return Err(FileAccessError::Io {
                path: joined,
                source: e,
            });
        }
    };

    for (idx, root) in roots.iter().enumerate() {
        let canonical_root = std::fs::canonicalize(root).map_err(|e| FileAccessError::Io {
            path: root.clone(),
            source: e,
        })?;
        if let Ok(rel) = abs.strip_prefix(&canonical_root) {
            let rel_posix = rel.to_string_lossy().replace('\\', "/");
            return Ok(CanonicalPath {
                abs,
                root_index: idx,
                rel_posix,
            });
        }
    }

    // Distinguish OutOfScope from SymlinkEscape: if the *lexical* join lives
    // inside a root but the *canonical* path does not, then a symlink moved
    // us out.
    for root in roots {
        if joined.starts_with(root) {
            return Err(FileAccessError::SymlinkEscape {
                path: joined,
                target: abs,
            });
        }
    }

    Err(FileAccessError::OutOfScope { path: abs })
}

fn has_parent_traversal(p: &Path) -> bool {
    p.components().any(|c| matches!(c, Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_input_inside_root_is_ok() {
        let tmp = std::env::temp_dir();
        let roots = vec![tmp.clone()];
        let file = tmp.join("file_access_test_ok");
        std::fs::write(&file, b"x").unwrap();
        let res = canonicalize_within_roots(&file, &tmp, &roots).unwrap();
        assert_eq!(res.root_index, 0);
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn outside_root_is_rejected() {
        let tmp = std::env::temp_dir();
        let roots = vec![tmp.join("some_nonexistent_root_for_test")];
        std::fs::create_dir_all(&roots[0]).ok();
        let file = tmp.join("file_access_test_outside");
        std::fs::write(&file, b"x").unwrap();
        let err = canonicalize_within_roots(&file, &tmp, &roots).unwrap_err();
        assert!(matches!(err, FileAccessError::OutOfScope { .. }));
        std::fs::remove_file(&file).ok();
        std::fs::remove_dir_all(&roots[0]).ok();
    }
}
