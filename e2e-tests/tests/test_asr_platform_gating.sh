#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

assert_contains() {
  local file="$1"
  local pattern="$2"
  if ! grep -Fq "$pattern" "$file"; then
    echo "[asr-platform-gating] missing pattern in $file: $pattern" >&2
    exit 1
  fi
}

assert_not_contains() {
  local file="$1"
  local pattern="$2"
  if grep -Fq "$pattern" "$file"; then
    echo "[asr-platform-gating] unexpected pattern in $file: $pattern" >&2
    exit 1
  fi
}

echo "[asr-platform-gating] checking Cargo target dependencies"
assert_contains crates/bifrost-admin/Cargo.toml "[target.'cfg(all(target_os = \"macos\", target_arch = \"aarch64\"))'.dependencies]"
assert_contains crates/bifrost-admin/Cargo.toml "qwen3-asr = \"0.2.2\""
assert_contains crates/bifrost-admin/Cargo.toml "sherpa-onnx = \"1.13.2\""
assert_not_contains crates/bifrost-admin/Cargo.toml "[target.'cfg(any(all(target_os = \"linux\""

echo "[asr-platform-gating] checking native cfg fences"
assert_contains crates/bifrost-admin/src/handlers/asr_jobs/diarization.rs "#[cfg(all(target_os = \"macos\", target_arch = \"aarch64\"))]"
assert_contains crates/bifrost-admin/src/handlers/asr_jobs/diarization.rs "#[cfg(not(all(target_os = \"macos\", target_arch = \"aarch64\")))]"
assert_contains crates/bifrost-admin/src/handlers/asr_jobs/voiceprint.rs "#[cfg(all(target_os = \"macos\", target_arch = \"aarch64\"))]"
assert_contains crates/bifrost-admin/src/handlers/asr_jobs/voiceprint.rs "#[cfg(not(all(target_os = \"macos\", target_arch = \"aarch64\")))]"
assert_contains crates/bifrost-admin/src/handlers/voice_stateful.rs "#[cfg(all(target_os = \"macos\", target_arch = \"aarch64\"))]"

echo "[asr-platform-gating] checking CLI help hiding fences"
assert_contains crates/bifrost-cli/src/cli.rs "not(all(target_os = \"macos\", target_arch = \"aarch64\"))"
assert_contains crates/bifrost-cli/src/cli.rs "command(hide = true)"

echo "[asr-platform-gating] checking cargo metadata platform filtering"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-platform-gating.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT
cargo metadata --locked --format-version=1 --filter-platform x86_64-unknown-linux-gnu > "$TMP_DIR/linux.json"
cargo metadata --locked --format-version=1 --filter-platform x86_64-apple-darwin > "$TMP_DIR/macos-x86.json"
cargo metadata --locked --format-version=1 --filter-platform aarch64-apple-darwin > "$TMP_DIR/macos-arm.json"
node - "$TMP_DIR/linux.json" "$TMP_DIR/macos-x86.json" "$TMP_DIR/macos-arm.json" <<'NODE'
const fs = require("fs");

function adminDeps(path) {
  const metadata = JSON.parse(fs.readFileSync(path, "utf8"));
  const packages = new Map(metadata.packages.map((pkg) => [pkg.id, pkg.name]));
  const admin = metadata.resolve.nodes.find((node) => packages.get(node.id) === "bifrost-admin");
  if (!admin) {
    throw new Error(`missing bifrost-admin resolve node in ${path}`);
  }
  return new Set(admin.deps.map((dep) => packages.get(dep.pkg) || dep.name));
}

const linux = adminDeps(process.argv[2]);
const macosX86 = adminDeps(process.argv[3]);
const macosArm = adminDeps(process.argv[4]);
for (const dep of ["qwen3-asr", "sherpa-onnx"]) {
  if (linux.has(dep)) {
    throw new Error(`${dep} must not resolve for x86_64-unknown-linux-gnu`);
  }
  if (macosX86.has(dep)) {
    throw new Error(`${dep} must not resolve for x86_64-apple-darwin`);
  }
  if (!macosArm.has(dep)) {
    throw new Error(`${dep} must resolve for aarch64-apple-darwin`);
  }
}
NODE

echo "[asr-platform-gating] PASS"
