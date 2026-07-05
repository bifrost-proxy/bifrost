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
UPDATED_SOURCE_APP="${TEST_ROOT}/source-updated/Bifrost.app"
INSTALL_DIR="${TEST_ROOT}/Applications"
mkdir -p "${SOURCE_APP}/Contents" "${UPDATED_SOURCE_APP}/Contents" "${INSTALL_DIR}"
cat >"${SOURCE_APP}/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>CFBundleShortVersionString</key>
  <string>9.9.9</string>
</dict>
</plist>
PLIST
cat >"${UPDATED_SOURCE_APP}/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>CFBundleShortVersionString</key>
  <string>10.0.0</string>
</dict>
</plist>
PLIST

DRY_RUN_OUTPUT="$("${BIFROST_BIN}" app install --package "${SOURCE_APP}" --app-dir "${INSTALL_DIR}" --version 9.9.9 --dry-run)"
echo "${DRY_RUN_OUTPUT}" | grep -q "Desktop app install target:"
echo "${DRY_RUN_OUTPUT}" | grep -q "Dry run: no files will be changed."
test ! -e "${INSTALL_DIR}/Bifrost.app"

if [[ "$(uname -s)" != "Darwin" ]]; then
  STATUS_JSON="$("${BIFROST_BIN}" native-app status --install-dir "${INSTALL_DIR}" --latest-version 9.9.9 --format json)"
  echo "${STATUS_JSON}" | grep -q '"installed": false'
  echo "${STATUS_JSON}" | grep -q '"supported": false'
  echo "${STATUS_JSON}" | grep -q 'Bifrost Native App is available only on macOS.'
  echo "macOS native app install CLI E2E passed on unsupported platform"
  exit 0
fi

"${BIFROST_BIN}" app install \
  --package "${SOURCE_APP}" \
  --app-dir "${INSTALL_DIR}" \
  --version 9.9.9 \
  -y

test -f "${INSTALL_DIR}/Bifrost.app/Contents/Info.plist"
STATUS_JSON="$("${BIFROST_BIN}" native-app status --install-dir "${INSTALL_DIR}" --latest-version 9.9.9 --format json)"
echo "${STATUS_JSON}" | grep -q '"installed": true'
echo "${STATUS_JSON}" | grep -q '"installed_version": "9.9.9"'
echo "${STATUS_JSON}" | grep -q '"needs_install": false'

UPDATE_NEEDED_JSON="$("${BIFROST_BIN}" native-app status --install-dir "${INSTALL_DIR}" --latest-version 10.0.0 --format json)"
echo "${UPDATE_NEEDED_JSON}" | grep -q '"installed": true'
echo "${UPDATE_NEEDED_JSON}" | grep -q '"installed_version": "9.9.9"'
echo "${UPDATE_NEEDED_JSON}" | grep -q '"needs_install": true'

"${BIFROST_BIN}" app upgrade \
  --package "${UPDATED_SOURCE_APP}" \
  --app-dir "${INSTALL_DIR}" \
  --source desktop \
  --no-cli \
  --version 10.0.0 \
  -y

UPDATED_STATUS_JSON="$("${BIFROST_BIN}" native-app status --install-dir "${INSTALL_DIR}" --latest-version 10.0.0 --format json)"
echo "${UPDATED_STATUS_JSON}" | grep -q '"installed": true'
echo "${UPDATED_STATUS_JSON}" | grep -q '"installed_version": "10.0.0"'
echo "${UPDATED_STATUS_JSON}" | grep -q '"needs_install": false'

"${BIFROST_BIN}" app uninstall --app-dir "${INSTALL_DIR}" -y
test ! -e "${INSTALL_DIR}/Bifrost.app"

echo "macOS native app install/update/uninstall CLI E2E passed"
