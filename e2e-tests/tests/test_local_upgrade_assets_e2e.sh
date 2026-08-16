#!/bin/bash
# Verify that --local-assets replaces only release discovery/download while the
# real extraction, atomic install, target verification, and cleanup path runs.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
WIX_TEMPLATE="${PROJECT_DIR}/desktop/src-tauri/wix/main.wxs"
TAURI_CONFIG="${PROJECT_DIR}/desktop/src-tauri/tauri.conf.json"

grep -Fq '"template": "wix/main.wxs"' "$TAURI_CONFIG"
grep -Fq '<Property Id="INSTALLDIR" Secure="yes" />' "$WIX_TEMPLATE"
grep -Fq '<Property Id="PREVINSTALLDIR">' "$WIX_TEMPLATE"
grep -Fq '<SetProperty Id="INSTALLDIR" Value="[PREVINSTALLDIR]" After="AppSearch" Sequence="both">' "$WIX_TEMPLATE"
if grep -Fq '<Property Id="INSTALLDIR">' "$WIX_TEMPLATE"; then
    echo '[FAIL] WiX AppSearch still writes directly into explicit INSTALLDIR' >&2
    exit 1
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
    echo "[SKIP] bifrost binary is unavailable: $BIFROST_BIN"
    exit 0
fi

case "$(uname -s)" in
    Darwin) os_target="apple-darwin" ;;
    Linux) os_target="unknown-linux-gnu" ;;
    MINGW*|MSYS*|CYGWIN*|Windows_NT)
        echo "[SKIP] fake executable fixture is Unix-only; Windows uses the real VM matrix"
        exit 0
        ;;
    *)
        echo "[SKIP] unsupported E2E host"
        exit 0
        ;;
esac

case "$(uname -m)" in
    arm64|aarch64) arch_target="aarch64" ;;
    x86_64|amd64) arch_target="x86_64" ;;
    *)
        echo "[SKIP] unsupported E2E architecture"
        exit 0
        ;;
esac

target="${arch_target}-${os_target}"
version="99.0.0"
temp_base="${TMPDIR:-/tmp}"
temp_base="${temp_base%/}"
test_root="$(mktemp -d "${temp_base}/bifrost-local-assets.XXXXXX")"
assets_dir="${test_root}/assets"
archive_root="${test_root}/archive/bifrost-v${version}-${target}"
install_target="${test_root}/installed/bifrost"
data_dir="${test_root}/data"

cleanup() {
    if [[ -n "${test_root:-}" && "$test_root" == "${temp_base}/bifrost-local-assets."* ]]; then
        rm -rf "$test_root"
    fi
}
trap cleanup EXIT

mkdir -p "$assets_dir" "$archive_root" "$(dirname "$install_target")" "$data_dir"
cat >"${archive_root}/bifrost" <<EOF
#!/bin/sh
echo 'bifrost ${version}'
EOF
chmod +x "${archive_root}/bifrost"
tar -C "${test_root}/archive" -czf \
    "${assets_dir}/bifrost-v${version}-${target}.tar.gz" \
    "bifrost-v${version}-${target}"

cat >"$install_target" <<'EOF'
#!/bin/sh
echo 'bifrost 0.0.1'
EOF
chmod +x "$install_target"

output="$({
    env -u BIFROST_EXTERNAL_CLI_WORKER \
        -u BIFROST_DETACHED_DAEMON_CHILD \
        BIFROST_DATA_DIR="$data_dir" \
        BIFROST_UPGRADE_TEST_INSTALL_TARGET="$install_target" \
        BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_APP=1 \
        BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_RESTART=1 \
        "$BIFROST_BIN" upgrade --local-assets "$assets_dir" -y
} 2>&1)"

grep -Fq "Using local release assets:" <<<"$output"
grep -Fq "Local target version: v${version}" <<<"$output"
grep -Fq "Upgrade completed successfully" <<<"$output"
[[ "$($install_target --version)" == "bifrost ${version}" ]]
[[ ! -e "${install_target}.backup" ]]

echo "[PASS] local assets drove the production extraction/install/verification path"
