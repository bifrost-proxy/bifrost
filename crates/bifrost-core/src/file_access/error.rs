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
