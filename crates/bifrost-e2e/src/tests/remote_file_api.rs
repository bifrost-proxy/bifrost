//! Integration coverage for the Remote File API policy primitives.
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
        // ---------------------------------------------------------------
        //  Phase 1 — read-only policy tests
        // ---------------------------------------------------------------
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
            "FileOp::as_str returns stable lowercase tokens for all ops (Phase 1+2+3)",
            "remote_file_api",
            || async {
                let cases = [
                    (FileOp::Read, "read"),
                    (FileOp::List, "list"),
                    (FileOp::Stat, "stat"),
                    (FileOp::Glob, "glob"),
                    (FileOp::Search, "search"),
                    (FileOp::Hash, "hash"),
                    (FileOp::Write, "write"),
                    (FileOp::Edit, "edit"),
                    (FileOp::Mkdir, "mkdir"),
                    (FileOp::Move, "move"),
                    (FileOp::Delete, "delete"),
                    (FileOp::ApplyPatch, "apply_patch"),
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
        // ---------------------------------------------------------------
        //  Phase 2 — write policy tests
        // ---------------------------------------------------------------
        TestCase::standalone(
            "remote_file_readonly_policy_rejects_write_op",
            "readonly policy denies Write operations",
            "remote_file_api",
            || async {
                let root = unique_tmp("ro-write");
                fs::create_dir_all(&root).map_err(|e| e.to_string())?;
                fs::write(root.join("test.txt"), b"data").map_err(|e| e.to_string())?;

                let policy = FileAccessPolicy::new_readonly("t", vec![root.clone()]);
                let err = policy
                    .check(std::path::Path::new("test.txt"), &root, FileOp::Write)
                    .err()
                    .ok_or_else(|| "expected error for write on readonly policy".to_string())?;

                let _ = fs::remove_dir_all(&root);
                if err.code() != "file.permission_denied" {
                    return Err(format!(
                        "expected file.permission_denied, got {}",
                        err.code()
                    ));
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "remote_file_write_policy_allows_write_op",
            "write policy accepts Write operations inside roots",
            "remote_file_api",
            || async {
                let root = unique_tmp("rw-write");
                fs::create_dir_all(&root).map_err(|e| e.to_string())?;
                fs::write(root.join("test.txt"), b"data").map_err(|e| e.to_string())?;

                let policy = FileAccessPolicy::new_read_write("t", vec![root.clone()]);
                let decision = policy
                    .check(std::path::Path::new("test.txt"), &root, FileOp::Write)
                    .map_err(|e| format!("expected ok, got {e}"))?;

                let _ = fs::remove_dir_all(&root);
                if decision.op != FileOp::Write {
                    return Err(format!("decision.op = {:?}, want Write", decision.op));
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "remote_file_write_policy_allows_all_write_ops",
            "write policy accepts Edit/Mkdir/Move/Delete/ApplyPatch ops",
            "remote_file_api",
            || async {
                let root = unique_tmp("rw-allops");
                fs::create_dir_all(&root).map_err(|e| e.to_string())?;
                fs::write(root.join("test.txt"), b"data").map_err(|e| e.to_string())?;

                let policy = FileAccessPolicy::new_read_write("t", vec![root.clone()]);
                for op in [
                    FileOp::Edit,
                    FileOp::Mkdir,
                    FileOp::Move,
                    FileOp::Delete,
                    FileOp::ApplyPatch,
                ] {
                    let result = policy.check(std::path::Path::new("test.txt"), &root, op);
                    if let Err(e) = result {
                        let _ = fs::remove_dir_all(&root);
                        return Err(format!("expected ok for {:?}, got {e}", op));
                    }
                }

                let _ = fs::remove_dir_all(&root);
                Ok(())
            },
        ),
        TestCase::standalone(
            "remote_file_write_policy_still_denies_secrets",
            "write policy still denies *.pem / *.key / .git/** for write ops",
            "remote_file_api",
            || async {
                let root = unique_tmp("rw-deny");
                fs::create_dir_all(root.join(".git")).map_err(|e| e.to_string())?;
                fs::write(root.join(".git/config"), b"[core]").map_err(|e| e.to_string())?;

                let policy = FileAccessPolicy::new_read_write("t", vec![root.clone()]);
                let err = policy
                    .check(std::path::Path::new(".git/config"), &root, FileOp::Write)
                    .err()
                    .ok_or_else(|| "expected deny for .git/config write".to_string())?;

                let _ = fs::remove_dir_all(&root);
                if err.code() != "file.permission_denied" {
                    return Err(format!(
                        "expected file.permission_denied, got {}",
                        err.code()
                    ));
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "remote_file_is_write_classification",
            "FileOp::is_write correctly classifies read vs write ops",
            "remote_file_api",
            || async {
                let read_ops = [
                    FileOp::Read,
                    FileOp::List,
                    FileOp::Stat,
                    FileOp::Glob,
                    FileOp::Search,
                    FileOp::Hash,
                ];
                let write_ops = [
                    FileOp::Write,
                    FileOp::Edit,
                    FileOp::Mkdir,
                    FileOp::Move,
                    FileOp::Delete,
                    FileOp::ApplyPatch,
                ];
                for op in read_ops {
                    if op.is_write() {
                        return Err(format!("{:?} should not be write", op));
                    }
                }
                for op in write_ops {
                    if !op.is_write() {
                        return Err(format!("{:?} should be write", op));
                    }
                }
                Ok(())
            },
        ),
    ]
}
