#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 <Bifrost.app> <rust-target>" >&2
  exit 2
fi

app_path="$1"
target="$2"
plist_buddy="${PLIST_BUDDY_BIN:-/usr/libexec/PlistBuddy}"
lipo_bin="${LIPO_BIN:-lipo}"

case "$target" in
  aarch64-apple-darwin)
    expected_arch="arm64"
    ;;
  x86_64-apple-darwin)
    expected_arch="x86_64"
    ;;
  *)
    echo "Unsupported macOS target: $target" >&2
    exit 2
    ;;
esac

info_plist="$app_path/Contents/Info.plist"
if [[ ! -f "$info_plist" ]]; then
  echo "Missing app Info.plist: $info_plist" >&2
  exit 1
fi

app_executable_name="$($plist_buddy -c 'Print :CFBundleExecutable' "$info_plist")"
app_executable="$app_path/Contents/MacOS/$app_executable_name"
sidecar="$app_path/Contents/Resources/resources/bin/bifrost"

for executable in "$app_executable" "$sidecar"; do
  if [[ ! -f "$executable" ]]; then
    echo "Missing bundled executable: $executable" >&2
    exit 1
  fi

  architectures="$($lipo_bin -archs "$executable")"
  if [[ "$architectures" != "$expected_arch" ]]; then
    echo "Architecture mismatch for $executable: expected=$expected_arch actual=$architectures target=$target" >&2
    exit 1
  fi
  echo "Validated macOS architecture: executable=$executable arch=$architectures target=$target"
done
