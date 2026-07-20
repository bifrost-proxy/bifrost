#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TEMP_DIR"' EXIT

RUNTIME_ROOT="$TEMP_DIR/moss-joint-runtime"
OUTPUT_ZIP="$TEMP_DIR/dist/moss-joint-runtime-v0.0.0-aarch64-apple-darwin.zip"
mkdir -p "$RUNTIME_ROOT/runtime/python/bin" "$RUNTIME_ROOT/runtime/site-packages" \
  "$RUNTIME_ROOT/model"
printf 'fixture\n' >"$RUNTIME_ROOT/runtime/site-packages/.fixture"

printf '%s\n' '#!/bin/sh' 'exec "$@"' >"$RUNTIME_ROOT/runtime/python/bin/python3.12-real"
ln -s python3.12-real "$RUNTIME_ROOT/runtime/python/bin/python3.12"
# The generated fixture must evaluate $1 at runtime.
# shellcheck disable=SC2016
printf '%s\n' '#!/bin/sh' 'test "${1:-}" = "--self-test"' \
  'echo "moss-mlx-runtime ok"' >"$RUNTIME_ROOT/runtime/moss_mlx_runner.py"
chmod +x "$RUNTIME_ROOT/runtime/python/bin/python3.12-real" \
  "$RUNTIME_ROOT/runtime/moss_mlx_runner.py"

for file in \
  added_tokens.json chat_template.jinja config.json generation_config.json \
  merges.txt model.safetensors.index.json preprocessor_config.json \
  processor_config.json special_tokens_map.json tokenizer.json \
  tokenizer_config.json vocab.json; do
  printf '{}\n' >"$RUNTIME_ROOT/model/$file"
done
printf 'fixture license\n' >"$RUNTIME_ROOT/MLX-AUDIO-LICENSE"
printf 'fixture model notice\n' >"$RUNTIME_ROOT/MODEL-NOTICE.txt"
printf 'forbidden weight\n' >"$RUNTIME_ROOT/model/model.safetensors"

if bash "$ROOT_DIR/scripts/ci/package-moss-release-runtime.sh" "$RUNTIME_ROOT" "$OUTPUT_ZIP"; then
  echo "Packager accepted a model weight that must be downloaded on demand" >&2
  exit 1
fi
rm "$RUNTIME_ROOT/model/model.safetensors"

printf 'forbidden sidecar\n' >"$RUNTIME_ROOT/model/._config.json"

if bash "$ROOT_DIR/scripts/ci/package-moss-release-runtime.sh" "$RUNTIME_ROOT" "$OUTPUT_ZIP"; then
  echo "Packager accepted a forbidden AppleDouble sidecar" >&2
  exit 1
fi
rm "$RUNTIME_ROOT/model/._config.json"

FAKE_BIN="$TEMP_DIR/fake-bin"
mkdir -p "$FAKE_BIN"
cat >"$FAKE_BIN/file" <<'SH'
#!/bin/sh
echo "$1: Mach-O 64-bit universal binary with 2 architectures"
SH
cat >"$FAKE_BIN/otool" <<'SH'
#!/bin/sh
binary="${2:?missing binary}"
echo "/Users/runner/work/bifrost/bifrost/dist/moss-joint-runtime/runtime/python/bin/python3.12 (architecture x86_64):"
echo "/Users/runner/work/bifrost/bifrost/dist/moss-joint-runtime/runtime/python/bin/python3.12 (architecture arm64):"
if [ "${FAKE_OTOOL_HOST_DEPENDENCY:-0}" = "1" ]; then
  printf '\t/Users/runner/hostedtoolcache/Python/3.12.10/arm64/lib/libpython3.12.dylib\n'
else
  printf '\t@rpath/Python3.framework/Versions/3.12/Python3\n'
fi
test -n "$binary"
SH
chmod +x "$FAKE_BIN/file" "$FAKE_BIN/otool"

PATH="$FAKE_BIN:$PATH" \
  bash "$ROOT_DIR/scripts/ci/package-moss-release-runtime.sh" "$RUNTIME_ROOT" "$OUTPUT_ZIP"

if FAKE_OTOOL_HOST_DEPENDENCY=1 PATH="$FAKE_BIN:$PATH" \
  bash "$ROOT_DIR/scripts/ci/package-moss-release-runtime.sh" "$RUNTIME_ROOT" "$OUTPUT_ZIP"; then
  echo "Packager accepted a build-host dylib dependency" >&2
  exit 1
fi

PATH="$FAKE_BIN:$PATH" \
  bash "$ROOT_DIR/scripts/ci/package-moss-release-runtime.sh" "$RUNTIME_ROOT" "$OUTPUT_ZIP"

test -s "$OUTPUT_ZIP"
test -s "$OUTPUT_ZIP.sha256"
VERIFY_DIR="$TEMP_DIR/verify"
ditto -x -k "$OUTPUT_ZIP" "$VERIFY_DIR"
test -L "$VERIFY_DIR/moss-joint-runtime/runtime/python/bin/python3.12"
test "$(readlink "$VERIFY_DIR/moss-joint-runtime/runtime/python/bin/python3.12")" = "python3.12-real"
if grep -Fq '._config.json' < <(unzip -Z1 "$OUTPUT_ZIP"); then
  echo "Fixture AppleDouble file leaked into the release archive" >&2
  exit 1
fi

echo "PASS: release workflow MOSS packaging fixture"
