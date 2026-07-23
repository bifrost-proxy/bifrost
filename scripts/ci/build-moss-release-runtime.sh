#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RUNTIME_VERSION="${1:?usage: build-moss-release-runtime.sh <runtime-version> <output-dir>}"
OUTPUT_DIR="${2:?usage: build-moss-release-runtime.sh <runtime-version> <output-dir>}"

SOURCE_COMMIT="64e8416c303fb3b3463dab8eb4ebd78c55a87c1a"
MODEL_COMMIT="90c3a1ab78fa56e47e1493ddea48e3ababaf2f71"
PYTHON_RELEASE="20260510"
PYTHON_ARCHIVE="cpython-3.12.13+20260510-aarch64-apple-darwin-install_only_stripped.tar.gz"
PYTHON_SHA256="55bc1a5edbc8ac4da0081f4f5731ed2d1ed10c57cb37a820b2a0dbc7cad742e9"
PYTHON_URL="https://github.com/astral-sh/python-build-standalone/releases/download/${PYTHON_RELEASE}/${PYTHON_ARCHIVE/+/%2B}"

SOURCE_DIR="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/mlx-audio"
RUNTIME_ROOT="$OUTPUT_DIR/moss-joint-runtime"
CACHE_DIR="${BIFROST_MOSS_RUNTIME_CACHE_DIR:-$ROOT_DIR/.ci-cache/moss-runtime}"
PYTHON_TARBALL="$CACHE_DIR/$PYTHON_ARCHIVE"
OUTPUT_ZIP="$OUTPUT_DIR/moss-joint-runtime-v${RUNTIME_VERSION}-aarch64-apple-darwin.zip"

mkdir -p "$CACHE_DIR" "$OUTPUT_DIR"
if [[ ! -f "$PYTHON_TARBALL" ]] ||
  ! printf '%s  %s\n' "$PYTHON_SHA256" "$PYTHON_TARBALL" | shasum -a 256 --check --status; then
  curl --fail --location --retry 5 --output "$PYTHON_TARBALL" "$PYTHON_URL"
fi
printf '%s  %s\n' "$PYTHON_SHA256" "$PYTHON_TARBALL" | shasum -a 256 --check

if [[ -e "$SOURCE_DIR" || -e "$RUNTIME_ROOT" ]]; then
  echo "MOSS runtime build inputs already exist; use a clean runner workspace" >&2
  exit 1
fi

git clone --filter=blob:none --no-checkout https://github.com/Blaizzy/mlx-audio.git "$SOURCE_DIR"
git -C "$SOURCE_DIR" fetch --depth 1 origin "$SOURCE_COMMIT"
git -C "$SOURCE_DIR" checkout --detach "$SOURCE_COMMIT"
git -C "$SOURCE_DIR" apply --unidiff-zero \
  "$ROOT_DIR/scripts/asr/mlx-audio-moss-quantized-conv.patch"

mkdir -p "$RUNTIME_ROOT/runtime" "$RUNTIME_ROOT/model"
tar -xzf "$PYTHON_TARBALL" -C "$RUNTIME_ROOT/runtime"
test -x "$RUNTIME_ROOT/runtime/python/bin/python3.12"

PYTHON="$RUNTIME_ROOT/runtime/python/bin/python3.12"
SITE_PACKAGES="$RUNTIME_ROOT/runtime/site-packages"
mkdir -p "$SITE_PACKAGES"
"$PYTHON" -m pip install --disable-pip-version-check --target "$SITE_PACKAGES" \
  -r "$ROOT_DIR/scripts/asr/moss-mlx-requirements.txt"
"$PYTHON" -m pip install --disable-pip-version-check --no-deps --target "$SITE_PACKAGES" \
  "$SOURCE_DIR"

cp "$ROOT_DIR/scripts/asr/moss_mlx_runner.py" "$RUNTIME_ROOT/runtime/moss_mlx_runner.py"
cp "$SOURCE_DIR/LICENSE" "$RUNTIME_ROOT/MLX-AUDIO-LICENSE"

MODEL_BASE="https://huggingface.co/majentik/MOSS-Transcribe-Diarize-MLX-8bit/resolve/${MODEL_COMMIT}"
for file in \
  added_tokens.json chat_template.jinja config.json generation_config.json \
  merges.txt model.safetensors.index.json preprocessor_config.json \
  processor_config.json special_tokens_map.json tokenizer.json \
  tokenizer_config.json vocab.json; do
  curl --fail --location --retry 5 --output "$RUNTIME_ROOT/model/$file" \
    "$MODEL_BASE/$file"
done
cp "$ROOT_DIR/scripts/asr/MOSS-MLX-MODEL-NOTICE.txt" "$RUNTIME_ROOT/MODEL-NOTICE.txt"

bash "$ROOT_DIR/scripts/ci/package-moss-release-runtime.sh" "$RUNTIME_ROOT" "$OUTPUT_ZIP"
