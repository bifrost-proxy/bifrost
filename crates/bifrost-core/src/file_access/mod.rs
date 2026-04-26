//! File access control primitives for the Remote Invoke File API (Phase 1).
//!
//! See `design/remote-invoke-file-api.md` for the complete design. This module
//! provides the server-side policy model + path canonicalization helpers that
//! the `bifrost-admin` remote_invoke executor will call **before** performing
//! any filesystem operation requested by a remote grant.
//!
//! # Non-goals (Phase 1)
//!
//! - Write/modify operations. Only read/list/stat/glob/search/hash are
//!   validated here; write enforcement lands in Phase 2 together with
//!   `apply_patch`.
//! - Distributed policy sync. The policy is loaded from the local admin
//!   config; sync is orthogonal and handled by `bifrost-sync` separately.
//!
//! # Layering
//!
//! ```text
//!   bifrost-admin (executor) ──▶ bifrost-core::file_access::FileAccessPolicy
//!                                 │
//!                                 ├─ path::canonicalize_within_roots
//!                                 ├─ matcher::{glob, deny}
//!                                 └─ error::FileAccessError
//! ```
//!
//! Keeping the policy in `bifrost-core` lets both the admin daemon and the
//! CLI (for `--dry-run` validation) share a single source of truth.

pub mod error;
pub mod matcher;
pub mod path;
pub mod policy;

pub use error::FileAccessError;
pub use matcher::{DenyMatcher, GlobMatcher};
pub use path::{canonicalize_within_roots, CanonicalPath};
pub use policy::{FileAccessPolicy, FileOp, PolicyDecision};
