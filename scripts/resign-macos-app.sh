#!/bin/bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <path-to-app-bundle>" >&2
  exit 1
fi

APP_PATH="$1"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
APP_ENTITLEMENTS="${REPO_ROOT}/desktop/src-tauri/Entitlements.plist"
WIDGET_ENTITLEMENTS="${REPO_ROOT}/desktop/macos-widget/BifrostStatusWidget.entitlements"
LOCAL_WIDGET_ENTITLEMENTS="${REPO_ROOT}/desktop/macos-widget/BifrostStatusWidget.local.entitlements"

if [[ ! -d "${APP_PATH}" ]]; then
  echo "App bundle not found: ${APP_PATH}" >&2
  exit 1
fi

IDENTITY="${APPLE_SIGNING_IDENTITY:-}"

if [[ -z "${IDENTITY}" ]]; then
  if [[ -n "${APPLE_CERTIFICATE:-}" ]]; then
    IDENTITY="$(security find-identity -v -p codesigning | awk -F '"' '/Developer ID Application/ { print $2; exit }')"
    if [[ -z "${IDENTITY}" ]]; then
      echo "Unable to detect a Developer ID Application signing identity." >&2
      exit 1
    fi
  else
    IDENTITY="-"
  fi
fi

if [[ "${IDENTITY}" == "-" ]]; then
  WIDGET_ENTITLEMENTS="${LOCAL_WIDGET_ENTITLEMENTS}"
fi

sign_args=(--force --sign "${IDENTITY}")
if [[ "${IDENTITY}" != "-" ]]; then
  sign_args+=(--options runtime)
fi

sign_executables_in_dir() {
  local dir="$1"
  local entitlements="${2:-}"
  if [[ ! -d "${dir}" ]]; then
    return 0
  fi

  while IFS= read -r -d '' file; do
    if [[ -n "${entitlements}" ]]; then
      codesign "${sign_args[@]}" --entitlements "${entitlements}" "${file}"
    else
      codesign "${sign_args[@]}" "${file}"
    fi
  done < <(find "${dir}" -type f -perm -111 -print0)
}

sign_executables_in_dir "${APP_PATH}/Contents/MacOS"
sign_executables_in_dir "${APP_PATH}/Contents/Resources/resources/bin" "${APP_ENTITLEMENTS}"

while IFS= read -r -d '' extension; do
  codesign "${sign_args[@]}" --entitlements "${WIDGET_ENTITLEMENTS}" "${extension}"
done < <(find "${APP_PATH}/Contents/PlugIns" -type d -name "*.appex" -print0 2>/dev/null || true)

codesign "${sign_args[@]}" --entitlements "${APP_ENTITLEMENTS}" "${APP_PATH}"
codesign --verify --deep --strict --verbose=4 "${APP_PATH}"

echo "Re-signed macOS app bundle: ${APP_PATH}"
