#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

export BIFROST_DISABLE_TRAY=1
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1

TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-native-app-install.XXXXXX")"
cleanup() {
  rm -rf "${TEST_ROOT}"
}
trap cleanup EXIT

BIFROST_BIN="${BIFROST_BIN:-${REPO_DIR}/target/debug/bifrost}"
if [[ ! -x "${BIFROST_BIN}" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost
fi

SOURCE_APP="${TEST_ROOT}/source/Bifrost.app"
INSTALL_DIR="${TEST_ROOT}/Applications"
mkdir -p "${SOURCE_APP}/Contents" "${INSTALL_DIR}"
cat >"${SOURCE_APP}/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>CFBundleShortVersionString</key>
  <string>9.9.9</string>
</dict>
</plist>
PLIST

DRY_RUN_OUTPUT="$("${BIFROST_BIN}" native-app install --source "${SOURCE_APP}" --install-dir "${INSTALL_DIR}" --dry-run)"
echo "${DRY_RUN_OUTPUT}" | grep -q '"dry_run":true'
test ! -e "${INSTALL_DIR}/Bifrost.app"

"${BIFROST_BIN}" native-app install \
  --source "${SOURCE_APP}" \
  --install-dir "${INSTALL_DIR}" \
  --latest-version 9.9.9 \
  -y

test -f "${INSTALL_DIR}/Bifrost.app/Contents/Info.plist"
STATUS_JSON="$("${BIFROST_BIN}" native-app status --install-dir "${INSTALL_DIR}" --latest-version 9.9.9 --format json)"
echo "${STATUS_JSON}" | grep -q '"installed": true'
echo "${STATUS_JSON}" | grep -q '"installed_version": "9.9.9"'

echo "macOS native app install CLI E2E passed"
