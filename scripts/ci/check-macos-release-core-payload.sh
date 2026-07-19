#!/usr/bin/env bash

set -euo pipefail

PAYLOAD_PATH="${1:?usage: check-macos-release-core-payload.sh <app-dir|binary|tar.gz|tar.xz|dmg>}"
MAX_BYTES="${BIFROST_MACOS_CORE_MAX_BYTES:-536870912}"

file_bytes() {
  if stat -f '%z' "$1" >/dev/null 2>&1; then
    stat -f '%z' "$1"
  else
    stat -c '%s' "$1"
  fi
}

if [[ -d "$PAYLOAD_PATH" ]]; then
  payload_bytes="$(du -sk "$PAYLOAD_PATH" | awk '{print $1 * 1024}')"
  payload_listing="$(
    cd "$(dirname "$PAYLOAD_PATH")"
    find "$(basename "$PAYLOAD_PATH")" -print
  )"
elif [[ -f "$PAYLOAD_PATH" ]]; then
  case "$PAYLOAD_PATH" in
    *.tar.gz | *.tar.xz)
      payload_bytes="$(file_bytes "$PAYLOAD_PATH")"
      payload_listing="$(tar -tf "$PAYLOAD_PATH")"
      ;;
    *.dmg)
      if ! command -v hdiutil >/dev/null 2>&1; then
        echo "hdiutil is required to inspect a macOS DMG" >&2
        exit 1
      fi
      mount_dir="$(mktemp -d)"
      cleanup_mount() {
        hdiutil detach "$mount_dir" -quiet >/dev/null 2>&1 || true
        rmdir "$mount_dir" >/dev/null 2>&1 || true
      }
      trap cleanup_mount EXIT
      hdiutil attach -nobrowse -readonly -mountpoint "$mount_dir" "$PAYLOAD_PATH" -quiet
      payload_bytes="$(file_bytes "$PAYLOAD_PATH")"
      payload_listing="$(cd "$mount_dir" && find . -print)"
      cleanup_mount
      trap - EXIT
      ;;
    *)
      payload_bytes="$(file_bytes "$PAYLOAD_PATH")"
      payload_listing="$(basename "$PAYLOAD_PATH")"
      ;;
  esac
else
  echo "macOS core payload does not exist: $PAYLOAD_PATH" >&2
  exit 1
fi

if ((payload_bytes > MAX_BYTES)); then
  echo "macOS core payload exceeds the release guard of $MAX_BYTES bytes: $payload_bytes bytes" >&2
  exit 1
fi

forbidden_pattern='(^|/)(moss_joint_mlx|moss-joint-runtime)(/|$)|(^|/)model\.safetensors$|\.safetensors$|(^|/)moss_mlx_runner\.py$|(^|/)site-packages(/|$)|(^|/)MLX-AUDIO-LICENSE$|(^|/)MOSS-MLX-MODEL-NOTICE\.txt$'
if grep -Ei "$forbidden_pattern" <<<"$payload_listing"; then
  echo "macOS core payload contains an ASR asset that must be downloaded on demand" >&2
  exit 1
fi

echo "PASS: macOS core payload stays lightweight ($payload_bytes bytes)"
