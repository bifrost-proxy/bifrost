//! Integration coverage for the Phase 1 Remote File API policy primitives.
//!
//! These tests exercise `bifrost_core::file_access` — the single choke
//! point every `file.*` remote invoke call flows through. The matching
//! shell test (`e2e-tests/tests/test_remote_file_api_e2e.sh`) covers
//! the CLI surface; real end-to-end transport (relay + target daemon)
//! lives in `human_tests/remote-invoke-file.md`.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use bifrost_core::file_access::{
    matcher::{DenyMatcher, GlobMatcher},
    policy::{FileAccessPolicy, FileOp},
};

use crate::runner::TestCase;

fn unique_tmp(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("bifrost-file-access-{}-{}", prefix, nanos))
}

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "remote_file_policy_allows_file_inside_root",
            "FileAccessPolicy::check succeeds for a regular file within the configured root",
            "remote_file_api",
            || async {
                let root = unique_tmp("ok");
                fs::create_dir_all(&root).map_err(|e| e.to_string())?;
                fs::write(root.join("README.md"), b"hello").map_err(|e| e.to_string())?;

                let policy = FileAccessPolicy::new_readonly("t", vec![root.clone()]);
                let decision = policy
                    .check(std::path::Path::new("README.md"), &root, FileOp::Read)
                    .map_err(|e| format!("expected ok, got {e}"))?;

                if decision.op != FileOp::Read {
                    let _ = fs::remove_dir_all(&root);
                    return Err(format!("decision.op = {:?}, want Read", decision.op));
                }
                if decision.max_read_bytes == 0 {
                    let _ = fs::remove_dir_all(&root);
                    return Err("max_read_bytes should default > 0".to_string());
                }

                let _ = fs::remove_dir_all(&root);
                Ok(())
            },
        ),
        TestCase::standalone(
            "remote_file_policy_denies_git_config",
            "default readonly policy denies .git/config via **/.git/** pattern",
            "remote_file_api",
            || async {
                let root = unique_tmp("deny");
                fs::create_dir_all(root.join(".git")).map_err(|e| e.to_string())?;
                fs::write(root.join(".git/config"), b"[core]").map_err(|e| e.to_string())?;

                let policy = FileAccessPolicy::new_readonly("t", vec![root.clone()]);
                let err = policy
                    .check(std::path::Path::new(".git/config"), &root, FileOp::Read)
                    .err()
                    .ok_or_else(|| "expected error, got ok".to_string())?;

                if err.code() != "file.permission_denied" {
                    let _ = fs::remove_dir_all(&root);
                    return Err(format!(
                        "expected file.permission_denied, got {}",
                        err.code()
                    ));
                }

                let _ = fs::remove_dir_all(&root);
                Ok(())
            },
        ),
        TestCase::standalone(
            "remote_file_policy_rejects_out_of_scope",
            "a path outside every configured root yields file.out_of_scope",
            "remote_file_api",
            || async {
                if cfg!(target_os = "windows") {
                    // /etc/passwd is meaningless on Windows; skip.
                    return Ok(());
                }
                let root = unique_tmp("scope");
                fs::create_dir_all(&root).map_err(|e| e.to_string())?;

                let policy = FileAccessPolicy::new_readonly("t", vec![root.clone()]);
                let err = policy
                    .check(std::path::Path::new("/etc/passwd"), &root, FileOp::Read)
                    .err()
                    .ok_or_else(|| "expected error, got ok".to_string())?;

                if err.code() != "file.out_of_scope" {
                    let _ = fs::remove_dir_all(&root);
                    return Err(format!("expected file.out_of_scope, got {}", err.code()));
                }

                let _ = fs::remove_dir_all(&root);
                Ok(())
            },
        ),
        TestCase::standalone(
            "remote_file_glob_matcher_root_relative",
            "GlobMatcher correctly matches root-relative POSIX paths with ** semantics",
            "remote_file_api",
            || async {
                let matcher =
                    GlobMatcher::new(&["src/**/*.rs".to_string()]).map_err(|e| e.to_string())?;

                if !matcher.is_match("src/main.rs") {
                    return Err("expected match for src/main.rs".to_string());
                }
                if !matcher.is_match("src/a/b/c.rs") {
                    return Err("expected match for src/a/b/c.rs".to_string());
                }
                if matcher.is_match("tests/smoke.rs") {
                    return Err("unexpected match for tests/smoke.rs".to_string());
                }
                if matcher.is_match("README.md") {
                    return Err("unexpected match for README.md".to_string());
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "remote_file_default_denies_cover_secrets_and_build_dirs",
            "default readonly denies cover *.key, *.pem, .git/, target/",
            "remote_file_api",
            || async {
                let policy = FileAccessPolicy::new_readonly("t", vec![std::env::temp_dir()]);
                let deny = DenyMatcher::new(&policy.denies).map_err(|e| e.to_string())?;

                for case in [
                    "id_rsa.key",
                    "certs/server.pem",
                    ".git/HEAD",
                    "a/.git/config",
                    "target/debug/foo",
                ] {
                    if deny.match_raw(case).is_none() {
                        return Err(format!("expected deny match for {case}"));
                    }
                }
                for case in ["src/main.rs", "README.md", "Cargo.toml"] {
                    if deny.match_raw(case).is_some() {
                        return Err(format!("unexpected deny match for {case}"));
                    }
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "remote_file_op_as_str_round_trip",
            "FileOp::as_str returns stable lowercase tokens for all Phase 1 ops",
            "remote_file_api",
            || async {
                let cases = [
                    (FileOp::Read, "read"),
                    (FileOp::List, "list"),
                    (FileOp::Stat, "stat"),
                    (FileOp::Glob, "glob"),
                    (FileOp::Search, "search"),
                    (FileOp::Hash, "hash"),
                ];
                for (op, want) in cases {
                    let got = op.as_str();
                    if got != want {
                        return Err(format!(
                            "FileOp::{:?}.as_str() = {}, want {}",
                            op, got, want
                        ));
                    }
                }
                Ok(())
            },
        ),
    ]
}
