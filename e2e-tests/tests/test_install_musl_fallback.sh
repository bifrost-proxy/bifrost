#!/bin/bash
#
# Bifrost install compatibility regression tests.
#
# These checks are intentionally offline: they validate target/package selection
# logic without downloading release artifacts.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

PASSED=0

pass() {
    echo "  ✓ $1"
    PASSED=$((PASSED + 1))
}

assert_eq() {
    local actual="$1"
    local expected="$2"
    local label="$3"
    if [[ "$actual" != "$expected" ]]; then
        echo "  ✗ $label"
        echo "    expected: $expected"
        echo "    actual:   $actual"
        exit 1
    fi
    pass "$label"
}

echo "==> install-binary.sh target fallback"

BIFROST_INSTALL_BINARY_SKIP_MAIN=1 source "$PROJECT_DIR/install-binary.sh"

get_glibc_version() { echo "2.28"; }
assert_eq "$(get_linux_target x86_64 gnu)" \
    "x86_64-unknown-linux-musl" \
    "glibc 2.28 selects x86_64 musl target"
assert_eq "$(get_linux_target aarch64 gnu)" \
    "aarch64-unknown-linux-musl" \
    "glibc 2.28 selects aarch64 musl target"

get_glibc_version() { echo "2.39"; }
assert_eq "$(get_linux_target x86_64 gnu)" \
    "x86_64-unknown-linux-gnu" \
    "glibc 2.39 keeps x86_64 gnu target"

get_glibc_version() { return 1; }
assert_eq "$(get_linux_target x86_64 gnu)" \
    "x86_64-unknown-linux-musl" \
    "unknown glibc version selects x86_64 musl target"

assert_eq "$(get_linux_target x86_64 musl)" \
    "x86_64-unknown-linux-musl" \
    "explicit musl remains musl"

echo "==> npm package fallback"

node <<'NODE'
const assert = require("assert");
const fs = require("fs");
const platform = require("./npm/bifrost/lib/platform.js");

function key(lddOutput, arch = "x64") {
  return platform.getPlatformKey({
    platform: "linux",
    arch,
    lddOutput,
    existsSync: () => false,
  });
}

assert.strictEqual(key("ldd (Debian GLIBC 2.28-10) 2.28"), "linux-x64-musl");
assert.strictEqual(
  platform.getPackageName({
    platform: "linux",
    arch: "x64",
    lddOutput: "ldd (Debian GLIBC 2.28-10) 2.28",
    existsSync: () => false,
  }),
  "@bifrost-proxy/bifrost-linux-x64-musl"
);
assert.strictEqual(key("ldd (GNU libc) 2.39"), "linux-x64-glibc");
assert.strictEqual(key("musl libc (x86_64)"), "linux-x64-musl");
assert.strictEqual(key("ldd (Debian GLIBC 2.28-10) 2.28", "arm"), "linux-arm-glibc");
assert.strictEqual(platform.parseGlibcVersion("ldd (GNU libc) 2.38"), "2.38");
assert.strictEqual(platform.versionLt("2.28", platform.MIN_GLIBC_VERSION), true);
assert.strictEqual(platform.versionLt("2.39", platform.MIN_GLIBC_VERSION), false);

const linuxX64 = JSON.parse(fs.readFileSync("./npm/bifrost-linux-x64/package.json", "utf8"));
const linuxArm64 = JSON.parse(fs.readFileSync("./npm/bifrost-linux-arm64/package.json", "utf8"));
const linuxX64Musl = JSON.parse(
  fs.readFileSync("./npm/bifrost-linux-x64-musl/package.json", "utf8")
);
const linuxArm64Musl = JSON.parse(
  fs.readFileSync("./npm/bifrost-linux-arm64-musl/package.json", "utf8")
);

assert.deepStrictEqual(linuxX64.libc, ["glibc"]);
assert.deepStrictEqual(linuxArm64.libc, ["glibc"]);
assert.strictEqual(Object.prototype.hasOwnProperty.call(linuxX64Musl, "libc"), false);
assert.strictEqual(Object.prototype.hasOwnProperty.call(linuxArm64Musl, "libc"), false);
NODE
pass "npm platform module selects musl for old glibc"
pass "npm musl platform packages are installable as static fallbacks on glibc"

echo ""
echo "Passed: $PASSED"
