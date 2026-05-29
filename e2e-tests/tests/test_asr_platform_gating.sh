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
assert_contains Cargo.toml "\"crates/bifrost-asr\""
assert_contains crates/bifrost-admin/Cargo.toml "bifrost-asr = { path = \"../bifrost-asr\", default-features = false, features = [\"core\"] }"
assert_contains crates/bifrost-admin/Cargo.toml "[target.'cfg(all(target_os = \"macos\", target_arch = \"aarch64\"))'.dependencies]"
assert_contains crates/bifrost-admin/Cargo.toml "bifrost-asr = { path = \"../bifrost-asr\", default-features = false, features = ["
assert_contains crates/bifrost-admin/Cargo.toml "\"full-local-asr\""
assert_contains crates/bifrost-asr/Cargo.toml "qwen3-asr = { version = \"0.2.2\", optional = true }"
assert_contains crates/bifrost-asr/Cargo.toml "sherpa-onnx = { version = \"1.13.2\", optional = true }"
assert_not_contains crates/bifrost-admin/Cargo.toml "qwen3-asr = \"0.2.2\""
assert_not_contains crates/bifrost-admin/Cargo.toml "sherpa-onnx = \"1.13.2\""
assert_not_contains crates/bifrost-admin/Cargo.toml "[target.'cfg(any(all(target_os = \"linux\""
assert_contains crates/bifrost-asr/src/lib.rs "pub mod planner;"
assert_contains crates/bifrost-asr/src/lib.rs "pub mod subtitle;"
assert_contains crates/bifrost-asr/src/lib.rs "pub mod offline;"
assert_contains crates/bifrost-asr/src/planner.rs "pub fn plan_asr_units"
assert_contains crates/bifrost-asr/src/offline.rs "pub fn write_offline_subtitle_artifacts"
assert_contains crates/bifrost-asr/src/subtitle.rs "pub fn render_srt"
assert_contains crates/bifrost-asr/src/subtitle.rs "pub fn render_vtt"

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

function packageNames(path) {
  const metadata = JSON.parse(fs.readFileSync(path, "utf8"));
  return new Set(metadata.packages.map((pkg) => pkg.name));
}

function directDeps(path, packageName) {
  const metadata = JSON.parse(fs.readFileSync(path, "utf8"));
  const packages = new Map(metadata.packages.map((pkg) => [pkg.id, pkg.name]));
  const node = metadata.resolve.nodes.find((node) => packages.get(node.id) === packageName);
  if (!node) {
    throw new Error(`missing ${packageName} resolve node in ${path}`);
  }
  return new Set(node.deps.map((dep) => packages.get(dep.pkg) || dep.name));
}

const linux = packageNames(process.argv[2]);
const macosX86 = packageNames(process.argv[3]);
const macosArm = packageNames(process.argv[4]);
const linuxAdminDeps = directDeps(process.argv[2], "bifrost-admin");
const macosX86AdminDeps = directDeps(process.argv[3], "bifrost-admin");
const macosArmAdminDeps = directDeps(process.argv[4], "bifrost-admin");
for (const deps of [linuxAdminDeps, macosX86AdminDeps, macosArmAdminDeps]) {
  if (!deps.has("bifrost-asr")) {
    throw new Error("bifrost-admin must depend on bifrost-asr as its ASR boundary crate");
  }
  for (const dep of ["qwen3-asr", "sherpa-onnx"]) {
    if (deps.has(dep)) {
      throw new Error(`bifrost-admin must not depend on ${dep} directly`);
    }
  }
}
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
