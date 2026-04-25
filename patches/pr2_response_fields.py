#!/usr/bin/env python3
"""PR#2 (cont.): populate new RemoteInvokeResponse large-output fields.

Changes to crates/bifrost-admin/src/remote_invoke/executor.rs:

1. Two query-path constructors become `..Default::default()` backed so new
   fields default to None / 0 without listing them verbatim.

2. The shell.exec constructor computes source-of-truth totals + SHA-256
   digests from the raw byte accumulators and fills the new fields.

3. Read loop continues to accumulate bytes into `stdout_preview`/`stderr_preview`
   via `append_truncated_bytes` (we still cap the inline preview by policy
   budget), BUT we ALSO track the total bytes observed and feed them to a
   running Sha256 hasher outside the cap. This keeps the response bounded
   while exposing the true size/digest to callers.

Idempotent: guarded by marker comments.
"""
import pathlib, sys, re

P = pathlib.Path("crates/bifrost-admin/src/remote_invoke/executor.rs")
MARKER_READLOOP = "// PR#2 source-of-truth hashing"

src = P.read_text()

# ---- (1) rewrite the two query-path RemoteInvokeResponse constructors ----
q_old_ok = """Ok(RemoteInvokeResponse {
                    exit_code: 0,
                    stdout: (!body.is_empty()).then_some(body),
                    stderr: None,
                    stdout_digest,
                    stderr_digest: None,
                    duration_ms,
                })"""
q_new_ok = """Ok(RemoteInvokeResponse {
                    exit_code: 0,
                    stdout: (!body.is_empty()).then_some(body),
                    stderr: None,
                    stdout_digest,
                    stderr_digest: None,
                    duration_ms,
                    ..Default::default()
                })"""

q_old_err = """Ok(RemoteInvokeResponse {
                    exit_code: -1,
                    stdout: None,
                    stderr: Some(stderr.clone()),
                    stdout_digest: None,
                    stderr_digest: Some(sha1_hex(&stderr)),
                    duration_ms,
                })"""
q_new_err = """Ok(RemoteInvokeResponse {
                    exit_code: -1,
                    stdout: None,
                    stderr: Some(stderr.clone()),
                    stdout_digest: None,
                    stderr_digest: Some(sha1_hex(&stderr)),
                    duration_ms,
                    ..Default::default()
                })"""

for old, new, tag in [(q_old_ok, q_new_ok, "query-ok"), (q_old_err, q_new_err, "query-err")]:
    if new in src:
        print(f"[skip] {tag} already patched")
    elif old in src:
        src = src.replace(old, new, 1)
        print(f"[ok] patched {tag}")
    else:
        print(f"[fail] {tag} block not found"); sys.exit(2)

# ---- (2) instrument the shell.exec read loop with a Sha256 hasher + totals ----
if MARKER_READLOOP in src:
    print("[skip] read-loop already instrumented")
else:
    # Insert hasher state declarations right after `let mut stderr_preview = Vec::new();`
    anchor_decl = "        let mut stderr_preview = Vec::new();\n"
    new_decl = anchor_decl + (
        "        // " + MARKER_READLOOP + ": track full byte totals + SHA-256 of\n"
        "        // the *complete* stdout/stderr streams, independent of the capped\n"
        "        // `stdout_preview` / `stderr_preview` inline buffers. This lets\n"
        "        // callers detect truncation and verify reassembled streams even\n"
        "        // when the inline preview was clipped by `max_output_bytes`.\n"
        "        let mut stdout_total_bytes: u64 = 0;\n"
        "        let mut stderr_total_bytes: u64 = 0;\n"
        "        let mut stdout_hasher = sha2::Sha256::new();\n"
        "        let mut stderr_hasher = sha2::Sha256::new();\n"
    )
    if anchor_decl not in src:
        print("[fail] anchor for hasher decl not found"); sys.exit(3)
    src = src.replace(anchor_decl, new_decl, 1)
    print("[ok] inserted hasher state")

    # Feed the hasher at every successful read of stdout
    feed_stdout_old = (
        "                        append_truncated_bytes(\n"
        "                            &mut stdout_preview,\n"
        "                            &stdout_buf[..read],\n"
        "                            policy.max_output_bytes,\n"
        "                        );\n"
        "                        on_stdout(String::from_utf8_lossy(&stdout_buf[..read]).into_owned()).await?;\n"
    )
    feed_stdout_new = (
        "                        // PR#2: hash + total BEFORE the cap so we retain truth even on overflow\n"
        "                        stdout_total_bytes += read as u64;\n"
        "                        sha2::Digest::update(&mut stdout_hasher, &stdout_buf[..read]);\n"
        "                        append_truncated_bytes(\n"
        "                            &mut stdout_preview,\n"
        "                            &stdout_buf[..read],\n"
        "                            policy.max_output_bytes,\n"
        "                        );\n"
        "                        on_stdout(String::from_utf8_lossy(&stdout_buf[..read]).into_owned()).await?;\n"
    )
    if feed_stdout_old in src:
        src = src.replace(feed_stdout_old, feed_stdout_new, 1)
        print("[ok] instrumented stdout read arm")
    elif feed_stdout_new in src:
        print("[skip] stdout read arm already instrumented")
    else:
        print("[fail] stdout read arm not found"); sys.exit(4)

    feed_stderr_old = (
        "                        append_truncated_bytes(\n"
        "                            &mut stderr_preview,\n"
        "                            &stderr_buf[..read],\n"
        "                            policy.max_output_bytes,\n"
        "                        );\n"
    )
    feed_stderr_new = (
        "                        // PR#2: hash + total BEFORE the cap\n"
        "                        stderr_total_bytes += read as u64;\n"
        "                        sha2::Digest::update(&mut stderr_hasher, &stderr_buf[..read]);\n"
        "                        append_truncated_bytes(\n"
        "                            &mut stderr_preview,\n"
        "                            &stderr_buf[..read],\n"
        "                            policy.max_output_bytes,\n"
        "                        );\n"
    )
    if feed_stderr_old in src:
        src = src.replace(feed_stderr_old, feed_stderr_new, 1)
        print("[ok] instrumented stderr read arm")
    elif feed_stderr_new in src:
        print("[skip] stderr read arm already instrumented")
    else:
        print("[fail] stderr read arm not found"); sys.exit(5)

# ---- (3) rewrite the shell.exec RemoteInvokeResponse constructor to use the new fields ----
shell_old = """Ok(RemoteInvokeResponse {
            exit_code: status.code().unwrap_or(-1),
            stdout: (!stdout.is_empty()).then_some(stdout.clone()),
            stderr: (!stderr.is_empty()).then_some(stderr.clone()),
            stdout_digest: (!stdout.is_empty()).then(|| sha1_hex(&stdout)),
            stderr_digest: (!stderr.is_empty()).then(|| sha1_hex(&stderr)),
            duration_ms: start.elapsed().as_millis() as u64,
        })"""
shell_new = """// PR#2: source-of-truth fields independent of the inline preview.
        let stdout_sha256_full_hex = {
            let digest = sha2::Digest::finalize(stdout_hasher);
            if stdout_total_bytes > 0 { Some(hex::encode(digest)) } else { None }
        };
        let stderr_sha256_full_hex = {
            let digest = sha2::Digest::finalize(stderr_hasher);
            if stderr_total_bytes > 0 { Some(hex::encode(digest)) } else { None }
        };
        let stdout_truncated_flag = (stdout_total_bytes as usize) > stdout_preview.len();
        let stderr_truncated_flag = (stderr_total_bytes as usize) > stderr_preview.len();
        Ok(RemoteInvokeResponse {
            exit_code: status.code().unwrap_or(-1),
            stdout: (!stdout.is_empty()).then_some(stdout.clone()),
            stderr: (!stderr.is_empty()).then_some(stderr.clone()),
            stdout_digest: (!stdout.is_empty()).then(|| sha1_hex(&stdout)),
            stderr_digest: (!stderr.is_empty()).then(|| sha1_hex(&stderr)),
            duration_ms: start.elapsed().as_millis() as u64,
            stdout_total_bytes: (stdout_total_bytes > 0).then_some(stdout_total_bytes),
            stderr_total_bytes: (stderr_total_bytes > 0).then_some(stderr_total_bytes),
            stdout_sha256_full: stdout_sha256_full_hex,
            stderr_sha256_full: stderr_sha256_full_hex,
            stdout_truncated: Some(stdout_truncated_flag),
            stderr_truncated: Some(stderr_truncated_flag),
        })"""
if shell_new.splitlines()[0] in src:
    print("[skip] shell.exec response block already rewritten")
elif shell_old in src:
    src = src.replace(shell_old, shell_new, 1)
    print("[ok] rewrote shell.exec response block")
else:
    print("[fail] shell.exec response block not found"); sys.exit(6)

# ---- (4) ensure sha2 + hex imports are present (add gated use lines at top-of-file) ----
# We access sha2::Digest, sha2::Sha256, and hex::encode; if the crate already
# imports them elsewhere the qualified paths work anyway. Nothing more to do.
P.write_text(src)
print(f"[done] bytes: {len(src)}")
