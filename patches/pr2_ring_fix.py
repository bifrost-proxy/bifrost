#!/usr/bin/env python3
"""PR#2 fixup: replace sha2::Sha256 + hex::encode with ring::digest::Context streaming."""
import pathlib, sys

p = pathlib.Path("crates/bifrost-admin/src/remote_invoke/executor.rs")
s = p.read_text()
orig = s

# 1) hasher init
s = s.replace(
    "        let mut stdout_hasher = sha2::Sha256::new();\n"
    "        let mut stderr_hasher = sha2::Sha256::new();",
    "        let mut stdout_hasher = ring::digest::Context::new(&ring::digest::SHA256);\n"
    "        let mut stderr_hasher = ring::digest::Context::new(&ring::digest::SHA256);",
)

# 2) update calls
s = s.replace(
    "sha2::Digest::update(&mut stdout_hasher, &stdout_buf[..read]);",
    "stdout_hasher.update(&stdout_buf[..read]);",
)
s = s.replace(
    "sha2::Digest::update(&mut stderr_hasher, &stderr_buf[..read]);",
    "stderr_hasher.update(&stderr_buf[..read]);",
)

# 3) finalize + hex
s = s.replace(
    "        let stdout_sha256_full_hex = {\n"
    "            let digest = sha2::Digest::finalize(stdout_hasher);\n"
    "            if stdout_total_bytes > 0 { Some(hex::encode(digest)) } else { None }",
    "        let stdout_sha256_full_hex = {\n"
    "            let digest = stdout_hasher.finish();\n"
    "            if stdout_total_bytes > 0 {\n"
    "                let mut out = String::with_capacity(64);\n"
    "                for b in digest.as_ref() {\n"
    "                    out.push_str(&format!(\"{b:02x}\"));\n"
    "                }\n"
    "                Some(out)\n"
    "            } else { None }",
)
s = s.replace(
    "        let stderr_sha256_full_hex = {\n"
    "            let digest = sha2::Digest::finalize(stderr_hasher);\n"
    "            if stderr_total_bytes > 0 { Some(hex::encode(digest)) } else { None }",
    "        let stderr_sha256_full_hex = {\n"
    "            let digest = stderr_hasher.finish();\n"
    "            if stderr_total_bytes > 0 {\n"
    "                let mut out = String::with_capacity(64);\n"
    "                for b in digest.as_ref() {\n"
    "                    out.push_str(&format!(\"{b:02x}\"));\n"
    "                }\n"
    "                Some(out)\n"
    "            } else { None }",
)

if s == orig:
    print("NO CHANGES — anchors missed", file=sys.stderr)
    sys.exit(2)

# sanity: no sha2/hex:: left
for bad in ("sha2::", "hex::encode"):
    if bad in s:
        print(f"RESIDUAL {bad!r} still present", file=sys.stderr)
        sys.exit(3)

p.write_text(s)
print("PATCHED OK")
