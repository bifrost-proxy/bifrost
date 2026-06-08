#!/usr/bin/env bash
set -euo pipefail

target="${1:-}"
link_mode="${2:-static}"

if [[ -z "${target}" ]]; then
  echo "usage: $0 <rust-target-triple> [static|shared]" >&2
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
version="${SHERPA_ONNX_VERSION:-}"
if [[ -z "${version}" ]]; then
  version="$(awk '
    $0 == "[[package]]" { in_pkg = 0 }
    $0 == "name = \"sherpa-onnx-sys\"" { in_pkg = 1 }
    in_pkg && $1 == "version" {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' "${repo_root}/Cargo.lock")"
fi

if [[ -z "${version}" ]]; then
  echo "failed to resolve sherpa-onnx-sys version from Cargo.lock" >&2
  exit 1
fi

case "${link_mode}:${target}" in
  static:x86_64-unknown-linux-gnu|static:x86_64-unknown-linux-musl)
    archive_name="sherpa-onnx-v${version}-linux-x64-static-lib.tar.bz2"
    ;;
  static:aarch64-unknown-linux-gnu|static:aarch64-unknown-linux-musl)
    archive_name="sherpa-onnx-v${version}-linux-aarch64-static-lib.tar.bz2"
    ;;
  static:x86_64-apple-darwin)
    archive_name="sherpa-onnx-v${version}-osx-x64-static-lib.tar.bz2"
    ;;
  static:aarch64-apple-darwin)
    archive_name="sherpa-onnx-v${version}-osx-arm64-static-lib.tar.bz2"
    ;;
  static:x86_64-pc-windows-msvc)
    archive_name="sherpa-onnx-v${version}-win-x64-static-MT-Release-lib.tar.bz2"
    ;;
  shared:x86_64-unknown-linux-gnu|shared:x86_64-unknown-linux-musl)
    archive_name="sherpa-onnx-v${version}-linux-x64-shared-lib.tar.bz2"
    ;;
  shared:aarch64-unknown-linux-gnu|shared:aarch64-unknown-linux-musl)
    archive_name="sherpa-onnx-v${version}-linux-aarch64-shared-cpu-lib.tar.bz2"
    ;;
  shared:x86_64-apple-darwin)
    archive_name="sherpa-onnx-v${version}-osx-x64-shared-lib.tar.bz2"
    ;;
  shared:aarch64-apple-darwin)
    archive_name="sherpa-onnx-v${version}-osx-arm64-shared-lib.tar.bz2"
    ;;
  shared:x86_64-pc-windows-msvc)
    archive_name="sherpa-onnx-v${version}-win-x64-shared-MT-Release-lib.tar.bz2"
    ;;
  *)
    echo "unsupported sherpa-onnx archive target/link mode: ${target} ${link_mode}" >&2
    exit 2
    ;;
esac

archive_dir="${SHERPA_ONNX_ARCHIVE_DIR:-${repo_root}/.ci-cache/sherpa-onnx}"
mkdir -p "${archive_dir}"

if [[ -n "${GITHUB_ENV:-}" ]]; then
  echo "SHERPA_ONNX_ARCHIVE_DIR=${archive_dir}" >> "${GITHUB_ENV}"
fi

archive_path="${archive_dir}/${archive_name}"
if [[ -f "${archive_path}" ]]; then
  if tar -tjf "${archive_path}" >/dev/null 2>&1; then
    echo "Using cached sherpa-onnx archive: ${archive_path}"
    exit 0
  fi
  echo "Cached sherpa-onnx archive is invalid; redownloading: ${archive_path}" >&2
  rm -f "${archive_path}"
fi

base_url="${SHERPA_ONNX_RELEASE_BASE_URL:-https://github.com/k2-fsa/sherpa-onnx/releases/download}"
url="${base_url}/v${version}/${archive_name}"
tmp_path="${archive_path}.tmp.$$"

cleanup() {
  rm -f "${tmp_path}"
}
trap cleanup EXIT

echo "Downloading sherpa-onnx archive: ${url}"
curl --fail --location --show-error \
  --retry "${SHERPA_ONNX_DOWNLOAD_RETRIES:-10}" \
  --retry-all-errors \
  --retry-delay "${SHERPA_ONNX_DOWNLOAD_RETRY_DELAY:-10}" \
  --connect-timeout "${SHERPA_ONNX_DOWNLOAD_CONNECT_TIMEOUT:-30}" \
  --max-time "${SHERPA_ONNX_DOWNLOAD_MAX_TIME:-600}" \
  --output "${tmp_path}" \
  "${url}"

tar -tjf "${tmp_path}" >/dev/null
mv "${tmp_path}" "${archive_path}"
echo "Prepared sherpa-onnx archive: ${archive_path}"
