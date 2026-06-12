//! Error types for `file_access`.
//!
//! These error codes are the wire-level `error.code` values returned to remote
//! callers (see design doc §5 "Error Catalog"). Keep the string form stable;
//! clients/CLIs branch on it.

use std::path::PathBuf;

use thiserror::Error;

/// Error returned by [`FileAccessPolicy`](crate::file_access::FileAccessPolicy)
/// and its helpers.
///
/// The `code()` method returns the stable wire code, while `Display` provides
/// a human-readable detail string.
#[derive(Debug, Error)]
pub enum FileAccessError {
    #[error("path is outside of any configured root: {path}")]
    OutOfScope { path: PathBuf },

    #[error("path matched a deny pattern ({pattern}): {path}")]
    DenyPattern { path: PathBuf, pattern: String },

    #[error("permission denied by policy: {reason}")]
    PermissionDenied { reason: &'static str },

    #[error("symlink target escapes the configured roots: {path} -> {target}")]
    SymlinkEscape { path: PathBuf, target: PathBuf },

    #[error("path is ignored by .gitignore: {path}")]
    IgnoredByGitignore { path: PathBuf },

    #[error("binary file read without --allow-binary: {path}")]
    BinaryNotAllowed { path: PathBuf },

    #[error("path not found: {path}")]
    NotFound { path: PathBuf },

    #[error("requested op {op} is not permitted by the active policy")]
    OpNotPermitted { op: &'static str },

    #[error("io error while resolving {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid glob pattern: {pattern} ({reason})")]
    InvalidGlob { pattern: String, reason: String },
}

impl FileAccessError {
    /// Stable wire-level error code. Matches the strings listed in the design
    /// doc §5 "Error Catalog".
    pub fn code(&self) -> &'static str {
        match self {
            FileAccessError::OutOfScope { .. } => "file.out_of_scope",
            FileAccessError::DenyPattern { .. } => "file.permission_denied",
            FileAccessError::PermissionDenied { .. } => "file.permission_denied",
            FileAccessError::SymlinkEscape { .. } => "file.symlink_escape",
            FileAccessError::IgnoredByGitignore { .. } => "file.ignored_by_gitignore",
            FileAccessError::BinaryNotAllowed { .. } => "file.binary_not_allowed",
            FileAccessError::NotFound { .. } => "file.not_found",
            FileAccessError::OpNotPermitted { .. } => "file.op_not_permitted",
            FileAccessError::Io { .. } => "file.io_error",
            FileAccessError::InvalidGlob { .. } => "file.invalid_glob",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_matches_each_variant() {
        let p = || PathBuf::from("/tmp/x");
        assert_eq!(
            FileAccessError::OutOfScope { path: p() }.code(),
            "file.out_of_scope"
        );
        assert_eq!(
            FileAccessError::DenyPattern {
                path: p(),
                pattern: "*.env".to_string()
            }
            .code(),
            "file.permission_denied"
        );
        assert_eq!(
            FileAccessError::PermissionDenied { reason: "ro" }.code(),
            "file.permission_denied"
        );
        assert_eq!(
            FileAccessError::SymlinkEscape {
                path: p(),
                target: p()
            }
            .code(),
            "file.symlink_escape"
        );
        assert_eq!(
            FileAccessError::IgnoredByGitignore { path: p() }.code(),
            "file.ignored_by_gitignore"
        );
        assert_eq!(
            FileAccessError::BinaryNotAllowed { path: p() }.code(),
            "file.binary_not_allowed"
        );
        assert_eq!(
            FileAccessError::NotFound { path: p() }.code(),
            "file.not_found"
        );
        assert_eq!(
            FileAccessError::OpNotPermitted { op: "write" }.code(),
            "file.op_not_permitted"
        );
        assert_eq!(
            FileAccessError::Io {
                path: p(),
                source: std::io::Error::other("boom")
            }
            .code(),
            "file.io_error"
        );
        assert_eq!(
            FileAccessError::InvalidGlob {
                pattern: "[".to_string(),
                reason: "bad".to_string()
            }
            .code(),
            "file.invalid_glob"
        );
    }

    #[test]
    fn display_includes_details() {
        let e = FileAccessError::OutOfScope {
            path: PathBuf::from("/etc/passwd"),
        };
        assert!(e.to_string().contains("/etc/passwd"));

        let e = FileAccessError::DenyPattern {
            path: PathBuf::from("/a/.env"),
            pattern: "*.env".to_string(),
        };
        let s = e.to_string();
        assert!(s.contains("*.env"));
        assert!(s.contains("/a/.env"));

        let e = FileAccessError::OpNotPermitted { op: "write" };
        assert!(e.to_string().contains("write"));
    }
}
