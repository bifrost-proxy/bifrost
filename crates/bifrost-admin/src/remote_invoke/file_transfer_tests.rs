//! Unit tests for [`super`] (the `file_transfer` module).
//!
//! Split into a sibling file (included via `#[path]`) to keep
//! `file_transfer.rs` under the repository's 1500-line ceiling.

use super::*;

fn rw_policy(root: &Path) -> FileAccessPolicy {
    FileAccessPolicy::new_read_write("t", vec![root.to_path_buf()])
}

fn zstd_accept() -> Vec<String> {
    vec!["zstd".to_string()]
}

#[test]
fn clamp_chunk_size_defaults_and_caps() {
    let tmp = std::env::temp_dir();
    let mut p = rw_policy(&tmp);
    p.transfer_chunk_max_bytes = 8 * 1024 * 1024;
    // omitted -> default
    assert_eq!(clamp_chunk_size(None, &p), DEFAULT_CHUNK_SIZE);
    // zero -> default
    assert_eq!(clamp_chunk_size(Some(0), &p), DEFAULT_CHUNK_SIZE);
    // under cap -> passthrough
    assert_eq!(clamp_chunk_size(Some(1024), &p), 1024);
    // over cap -> clamped
    assert_eq!(
        clamp_chunk_size(Some(100 * 1024 * 1024), &p),
        8 * 1024 * 1024
    );
}

#[test]
fn random_id_is_hex_and_unique() {
    let a = random_id();
    let b = random_id();
    assert_eq!(a.len(), 32);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, b);
}

#[test]
fn sha256_hex_matches_known_vector() {
    // sha256("") = e3b0c442...
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn encode_chunk_compresses_compressible_and_skips_incompressible() {
    // Highly compressible: zstd should win and tag zstd.
    let compressible = vec![0u8; 64 * 1024];
    let (payload, enc) = encode_chunk(&compressible, true);
    assert_eq!(enc, "zstd");
    assert!(payload.len() < compressible.len());
    let back = decode_chunk(&payload, Some("zstd"), 64 * 1024).unwrap();
    assert_eq!(back, compressible);

    // Incompressible (random): fall back to raw / none, no inflation.
    use ring::rand::{SecureRandom, SystemRandom};
    let mut incompressible = vec![0u8; 4096];
    SystemRandom::new().fill(&mut incompressible).unwrap();
    let (payload2, enc2) = encode_chunk(&incompressible, true);
    assert_eq!(enc2, "none");
    assert_eq!(payload2, incompressible);

    // allow_zstd = false always yields raw / none.
    let (payload3, enc3) = encode_chunk(&compressible, false);
    assert_eq!(enc3, "none");
    assert_eq!(payload3, compressible);
}

#[test]
fn decode_chunk_rejects_unknown_encoding() {
    let err = decode_chunk(b"x", Some("lz4"), 1024).unwrap_err();
    assert!(format!("{:?}", err).contains("file.invalid_args"));
}

#[tokio::test]
async fn upload_roundtrip_commit_verifies_sha() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("out.bin");
    let policy = rw_policy(dir.path());
    let decision = policy
        .check(
            &target,
            dir.path(),
            bifrost_core::file_access::FileOp::Upload,
        )
        .unwrap();
    let data = b"hello chunked world".repeat(1000);
    let sha = sha256_hex(&data);

    let begin = handle_upload_begin(
        &decision,
        &policy,
        Some(data.len() as u64),
        Some(&sha),
        Some(8),
        Some(true),
        false,
        None,
    )
    .await
    .unwrap();
    let id = begin["upload_id"].as_str().unwrap().to_string();
    let chunk = begin["effective_chunk_size"].as_u64().unwrap() as usize;
    assert_eq!(begin["received_offset"].as_u64().unwrap(), 0);
    assert!(!begin["already_complete"].as_bool().unwrap());

    let mut offset = 0u64;
    for piece in data.chunks(chunk) {
        let res = upload_chunk(TransferOpParams {
            transfer_id: Some(id.clone()),
            offset: Some(offset),
            length: None,
            chunk_b64: Some(b64_encode(piece)),
            chunk_sha256: Some(sha256_hex(piece)),
            chunk_encoding: Some("none".into()),
            total_sha256: None,
        })
        .await
        .unwrap();
        offset = res["next_offset"].as_u64().unwrap();
    }
    assert_eq!(offset, data.len() as u64);

    let commit = upload_commit(TransferOpParams {
        transfer_id: Some(id.clone()),
        offset: None,
        length: None,
        chunk_b64: None,
        chunk_sha256: None,
        chunk_encoding: None,
        total_sha256: Some(sha.clone()),
    })
    .await
    .unwrap();
    assert_eq!(commit["sha256"].as_str().unwrap(), sha);
    let on_disk = fs::read(&target).await.unwrap();
    assert_eq!(on_disk, data);
}

#[tokio::test]
async fn upload_out_of_order_chunks_land_in_order() {
    // Pipelined callers may deliver chunks out of order; the reorder buffer
    // must still land a contiguous, byte-correct file.
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("ooo.bin");
    let policy = rw_policy(dir.path());
    let decision = policy
        .check(
            &target,
            dir.path(),
            bifrost_core::file_access::FileOp::Upload,
        )
        .unwrap();
    let chunk = 1024usize;
    let data: Vec<u8> = (0..chunk * 5).map(|i| (i % 251) as u8).collect();
    let sha = sha256_hex(&data);
    let begin = handle_upload_begin(
        &decision,
        &policy,
        Some(data.len() as u64),
        Some(&sha),
        Some(chunk as u64),
        Some(true),
        false,
        Some(&zstd_accept()),
    )
    .await
    .unwrap();
    let id = begin["upload_id"].as_str().unwrap().to_string();
    assert_eq!(begin["chunk_encoding"].as_str().unwrap(), "zstd");

    // Send chunks 4,3,2,1 first (all ahead of the frontier -> buffered),
    // each compressed adaptively, then chunk 0 which unblocks the drain.
    let order = [4usize, 3, 2, 1, 0];
    let mut last_received = 0u64;
    for &i in &order {
        let start = i * chunk;
        let piece = &data[start..start + chunk];
        let (payload, enc) = encode_chunk(piece, true);
        let res = upload_chunk(TransferOpParams {
            transfer_id: Some(id.clone()),
            offset: Some(start as u64),
            length: None,
            chunk_b64: Some(b64_encode(&payload)),
            chunk_sha256: Some(sha256_hex(piece)),
            chunk_encoding: Some(enc.to_string()),
            total_sha256: None,
        })
        .await
        .unwrap();
        last_received = res["received_offset"].as_u64().unwrap();
    }
    // After chunk 0, the whole buffer drains contiguously.
    assert_eq!(last_received, data.len() as u64);

    let commit = upload_commit(TransferOpParams {
        transfer_id: Some(id.clone()),
        offset: None,
        length: None,
        chunk_b64: None,
        chunk_sha256: None,
        chunk_encoding: None,
        total_sha256: Some(sha.clone()),
    })
    .await
    .unwrap();
    assert_eq!(commit["sha256"].as_str().unwrap(), sha);
    assert_eq!(fs::read(&target).await.unwrap(), data);
}

#[tokio::test]
async fn upload_begin_skips_when_identical() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("same.bin");
    let data = b"already there".repeat(100);
    fs::write(&target, &data).await.unwrap();
    let sha = sha256_hex(&data);
    let policy = rw_policy(dir.path());
    let decision = policy
        .check(
            &target,
            dir.path(),
            bifrost_core::file_access::FileOp::Upload,
        )
        .unwrap();
    let begin = handle_upload_begin(
        &decision,
        &policy,
        Some(data.len() as u64),
        Some(&sha),
        None,
        Some(true),
        false,
        None,
    )
    .await
    .unwrap();
    assert!(begin["already_complete"].as_bool().unwrap());
    assert_eq!(
        begin["received_offset"].as_u64().unwrap(),
        data.len() as u64
    );
    assert_eq!(begin["sha256"].as_str().unwrap(), sha);
}

#[tokio::test]
async fn upload_chunk_rejects_bad_sha() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("out.bin");
    let policy = rw_policy(dir.path());
    let decision = policy
        .check(
            &target,
            dir.path(),
            bifrost_core::file_access::FileOp::Upload,
        )
        .unwrap();
    let data = vec![7u8; 4096];
    let sha = sha256_hex(&data);
    let begin = handle_upload_begin(
        &decision,
        &policy,
        Some(data.len() as u64),
        Some(&sha),
        Some(1024),
        Some(true),
        false,
        None,
    )
    .await
    .unwrap();
    let id = begin["upload_id"].as_str().unwrap().to_string();

    // Bad per-chunk sha.
    let bad = upload_chunk(TransferOpParams {
        transfer_id: Some(id.clone()),
        offset: Some(0),
        length: None,
        chunk_b64: Some(b64_encode(&data[..1024])),
        chunk_sha256: Some("deadbeef".into()),
        chunk_encoding: Some("none".into()),
        total_sha256: None,
    })
    .await;
    assert!(format!("{:?}", bad.unwrap_err()).contains("file.sha_mismatch"));
}

#[tokio::test]
async fn download_roundtrip_chunks_and_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let data = b"download me please".repeat(500);
    fs::write(&src, &data).await.unwrap();
    let policy = rw_policy(dir.path());
    let decision = policy
        .check(
            &src,
            dir.path(),
            bifrost_core::file_access::FileOp::Download,
        )
        .unwrap();
    let begin = handle_download_begin(&decision, &policy, Some(4096), Some(&zstd_accept()))
        .await
        .unwrap();
    // The caller-requested chunk size must be honoured (clamped) rather
    // than silently replaced by the server default — otherwise a large
    // default frame overruns the Relay's per-frame body limit.
    assert_eq!(begin["effective_chunk_size"].as_u64().unwrap(), 4096);
    assert_eq!(begin["content_encoding"].as_str().unwrap(), "zstd");
    let id = begin["download_id"].as_str().unwrap().to_string();
    let total = begin["total_size"].as_u64().unwrap();
    let whole_sha = begin["total_sha256"].as_str().unwrap().to_string();

    let mut got = Vec::new();
    let mut offset = 0u64;
    while offset < total {
        let res = download_chunk(TransferOpParams {
            transfer_id: Some(id.clone()),
            offset: Some(offset),
            length: Some(4096),
            chunk_b64: None,
            chunk_sha256: None,
            chunk_encoding: None,
            total_sha256: None,
        })
        .await
        .unwrap();
        let payload = b64_decode(res["chunk_b64"].as_str().unwrap()).unwrap();
        let enc = res["chunk_encoding"].as_str();
        let piece = decode_chunk(&payload, enc, 4096).unwrap();
        assert_eq!(res["chunk_sha256"].as_str().unwrap(), sha256_hex(&piece));
        got.extend_from_slice(&piece);
        offset = res["next_offset"].as_u64().unwrap();
    }
    assert_eq!(got, data);
    assert_eq!(sha256_hex(&got), whole_sha);
}

#[tokio::test]
async fn upload_begin_rejects_oversize() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("big.bin");
    let mut policy = rw_policy(dir.path());
    policy.max_transfer_bytes = 10;
    let decision = policy
        .check(
            &target,
            dir.path(),
            bifrost_core::file_access::FileOp::Upload,
        )
        .unwrap();
    let err = handle_upload_begin(
        &decision,
        &policy,
        Some(1000),
        Some("ab"),
        None,
        Some(true),
        false,
        None,
    )
    .await
    .unwrap_err();
    assert!(format!("{:?}", err).contains("file.size_too_large"));
}

// ---- P1-#5 small-file fast path: exhaustive edge coverage ----

fn small_policy(root: &Path) -> FileAccessPolicy {
    let mut p = rw_policy(root);
    // Modest ceiling so "at budget" / "over budget" are cheap to exercise.
    p.transfer_chunk_max_bytes = 4096;
    p.max_transfer_bytes = 1024 * 1024;
    p
}

async fn upload_small_call(
    policy: &FileAccessPolicy,
    target: &Path,
    cwd: &Path,
    raw: &[u8],
    encoding: &str,
    overwrite: Option<bool>,
    create_parents: bool,
) -> Result<Value> {
    let decision = policy
        .check(target, cwd, bifrost_core::file_access::FileOp::Upload)
        .unwrap();
    let sha = sha256_hex(raw);
    let (payload, _enc) = if encoding == "zstd" {
        encode_chunk(raw, true)
    } else {
        (raw.to_vec(), "none")
    };
    handle_upload_small(
        &decision,
        policy,
        Some(raw.len() as u64),
        Some(&sha),
        Some(&b64_encode(&payload)),
        Some(encoding),
        overwrite,
        create_parents,
    )
    .await
}

#[tokio::test]
async fn upload_small_writes_and_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let policy = small_policy(dir.path());
    let target = dir.path().join("small.bin");
    let data = b"a modest file that fits in one frame".repeat(10);
    let res = upload_small_call(
        &policy,
        &target,
        dir.path(),
        &data,
        "none",
        Some(true),
        false,
    )
    .await
    .unwrap();
    assert_eq!(res["bytes_written"].as_u64().unwrap(), data.len() as u64);
    assert_eq!(res["sha256"].as_str().unwrap(), sha256_hex(&data));
    assert!(!res["already_complete"].as_bool().unwrap());
    assert_eq!(fs::read(&target).await.unwrap(), data);
    // No .part debris left behind.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".part"))
        .collect();
    assert!(leftovers.is_empty(), "stray .part files: {:?}", leftovers);
}

#[tokio::test]
async fn upload_small_handles_empty_and_one_byte() {
    let dir = tempfile::tempdir().unwrap();
    let policy = small_policy(dir.path());

    // 0-byte file.
    let empty = dir.path().join("empty.bin");
    let res = upload_small_call(&policy, &empty, dir.path(), b"", "none", Some(true), false)
        .await
        .unwrap();
    assert_eq!(res["bytes_written"].as_u64().unwrap(), 0);
    assert_eq!(fs::read(&empty).await.unwrap(), b"");

    // 1-byte file.
    let one = dir.path().join("one.bin");
    let res = upload_small_call(&policy, &one, dir.path(), b"x", "none", Some(true), false)
        .await
        .unwrap();
    assert_eq!(res["bytes_written"].as_u64().unwrap(), 1);
    assert_eq!(fs::read(&one).await.unwrap(), b"x");
}

#[tokio::test]
async fn upload_small_boundary_at_budget_and_over() {
    let dir = tempfile::tempdir().unwrap();
    let policy = small_policy(dir.path());
    let budget = clamp_chunk_size(None, &policy).max(policy.transfer_chunk_max_bytes);

    // Exactly at budget: accepted.
    let at = dir.path().join("at.bin");
    let data_at = vec![7u8; budget as usize];
    let res = upload_small_call(
        &policy,
        &at,
        dir.path(),
        &data_at,
        "none",
        Some(true),
        false,
    )
    .await
    .unwrap();
    assert_eq!(res["bytes_written"].as_u64().unwrap(), budget);

    // budget + 1: rejected with precondition_failed (must use chunked).
    let over = dir.path().join("over.bin");
    let data_over = vec![7u8; budget as usize + 1];
    let err = upload_small_call(
        &policy,
        &over,
        dir.path(),
        &data_over,
        "none",
        Some(true),
        false,
    )
    .await
    .unwrap_err();
    assert!(format!("{:?}", err).contains("file.precondition_failed"));
}

#[tokio::test]
async fn upload_small_rejects_oversize_before_budget() {
    let dir = tempfile::tempdir().unwrap();
    let mut policy = small_policy(dir.path());
    policy.max_transfer_bytes = 8; // smaller than the frame budget
    let target = dir.path().join("big.bin");
    let data = vec![1u8; 100];
    let err = upload_small_call(
        &policy,
        &target,
        dir.path(),
        &data,
        "none",
        Some(true),
        false,
    )
    .await
    .unwrap_err();
    assert!(format!("{:?}", err).contains("file.size_too_large"));
}

#[tokio::test]
async fn upload_small_rejects_bad_sha_and_wrong_size() {
    let dir = tempfile::tempdir().unwrap();
    let policy = small_policy(dir.path());
    let target = dir.path().join("x.bin");
    let decision = policy
        .check(
            &target,
            dir.path(),
            bifrost_core::file_access::FileOp::Upload,
        )
        .unwrap();

    // Declared sha does not match the payload bytes.
    let raw = b"hello".to_vec();
    let bad = handle_upload_small(
        &decision,
        &policy,
        Some(raw.len() as u64),
        Some("deadbeef"),
        Some(&b64_encode(&raw)),
        Some("none"),
        Some(true),
        false,
    )
    .await
    .unwrap_err();
    assert!(format!("{:?}", bad).contains("file.sha_mismatch"));

    // Declared total_size disagrees with the decoded length.
    let wrong = handle_upload_small(
        &decision,
        &policy,
        Some(999),
        Some(&sha256_hex(&raw)),
        Some(&b64_encode(&raw)),
        Some("none"),
        Some(true),
        false,
    )
    .await
    .unwrap_err();
    assert!(format!("{:?}", wrong).contains("file.precondition_failed"));
    // Nothing was published on either failure.
    assert!(!fs::try_exists(&target).await.unwrap());
}

#[tokio::test]
async fn upload_small_zstd_payload_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let policy = small_policy(dir.path());
    let target = dir.path().join("z.bin");
    // Highly compressible so the payload is genuinely zstd-encoded.
    let data = vec![0u8; 2048];
    let res = upload_small_call(
        &policy,
        &target,
        dir.path(),
        &data,
        "zstd",
        Some(true),
        false,
    )
    .await
    .unwrap();
    assert_eq!(res["sha256"].as_str().unwrap(), sha256_hex(&data));
    assert_eq!(fs::read(&target).await.unwrap(), data);
}

#[tokio::test]
async fn upload_small_rejects_unknown_encoding() {
    let dir = tempfile::tempdir().unwrap();
    let policy = small_policy(dir.path());
    let target = dir.path().join("u.bin");
    let decision = policy
        .check(
            &target,
            dir.path(),
            bifrost_core::file_access::FileOp::Upload,
        )
        .unwrap();
    let raw = b"payload".to_vec();
    let err = handle_upload_small(
        &decision,
        &policy,
        Some(raw.len() as u64),
        Some(&sha256_hex(&raw)),
        Some(&b64_encode(&raw)),
        Some("lz4"),
        Some(true),
        false,
    )
    .await
    .unwrap_err();
    assert!(format!("{:?}", err).contains("file.invalid_args"));
}

#[tokio::test]
async fn upload_small_overwrite_gating_and_skip_identical() {
    let dir = tempfile::tempdir().unwrap();
    let policy = small_policy(dir.path());
    let target = dir.path().join("dup.bin");
    let data = b"first content".to_vec();
    fs::write(&target, &data).await.unwrap();

    // overwrite = false with differing content -> precondition_failed.
    let other = b"different content".to_vec();
    let err = upload_small_call(
        &policy,
        &target,
        dir.path(),
        &other,
        "none",
        Some(false),
        false,
    )
    .await
    .unwrap_err();
    assert!(format!("{:?}", err).contains("file.precondition_failed"));
    // Original untouched.
    assert_eq!(fs::read(&target).await.unwrap(), data);

    // Identical content -> skip-if-identical short-circuit even with
    // overwrite disabled (no write attempted).
    let res = upload_small_call(
        &policy,
        &target,
        dir.path(),
        &data,
        "none",
        Some(false),
        false,
    )
    .await
    .unwrap();
    assert!(res["already_complete"].as_bool().unwrap());
}

#[tokio::test]
async fn upload_small_create_parents() {
    let dir = tempfile::tempdir().unwrap();
    let policy = small_policy(dir.path());
    let nested = dir.path().join("a/b/c/deep.bin");
    let data = b"nested".to_vec();

    // Without create_parents the missing dir is reported.
    let err = upload_small_call(
        &policy,
        &nested,
        dir.path(),
        &data,
        "none",
        Some(true),
        false,
    )
    .await
    .unwrap_err();
    assert!(format!("{:?}", err).contains("file.not_found"));

    // With create_parents the tree is materialized and the file written.
    let res = upload_small_call(
        &policy,
        &nested,
        dir.path(),
        &data,
        "none",
        Some(true),
        true,
    )
    .await
    .unwrap();
    assert_eq!(res["bytes_written"].as_u64().unwrap(), data.len() as u64);
    assert_eq!(fs::read(&nested).await.unwrap(), data);
}

#[test]
fn part_file_name_is_char_safe_for_non_hex_sha() {
    // A multi-byte / short sha must never panic on a byte-boundary slice.
    let n = part_file_name("\u{e9}");
    assert!(n.starts_with(".bifrost-upload."));
    let long = part_file_name(&"\u{3042}".repeat(40));
    assert!(long.contains("\u{3042}"));
    // Standard 64-hex sha is truncated to 32 chars.
    let hex = "a".repeat(64);
    assert_eq!(
        part_file_name(&hex),
        format!(".bifrost-upload.{}.part", "a".repeat(32))
    );
}
