#!/usr/bin/env bash

set -euo pipefail

RUNTIME_ROOT="${1:?usage: package-moss-release-runtime.sh <runtime-root> <output-zip>}"
OUTPUT_ZIP="${2:?usage: package-moss-release-runtime.sh <runtime-root> <output-zip>}"

required_paths=(
  runtime/python/bin/python3.12
  runtime/site-packages
  runtime/moss_mlx_runner.py
  model/added_tokens.json
  model/chat_template.jinja
  model/config.json
  model/generation_config.json
  model/merges.txt
  model/model.safetensors.index.json
  model/preprocessor_config.json
  model/processor_config.json
  model/special_tokens_map.json
  model/tokenizer.json
  model/tokenizer_config.json
  model/vocab.json
  MLX-AUDIO-LICENSE
  MODEL-NOTICE.txt
)

for relative in "${required_paths[@]}"; do
  if [[ ! -e "$RUNTIME_ROOT/$relative" ]]; then
    echo "Missing MOSS release runtime input: $relative" >&2
    exit 1
  fi
done

if find "$RUNTIME_ROOT/model" -type f -name '*.safetensors' -print -quit | grep -q .; then
  echo "MOSS model weights must be downloaded on demand, not bundled in the runtime asset" >&2
  exit 1
fi

PYTHONHOME="$RUNTIME_ROOT/runtime/python" \
  PYTHONPATH="$RUNTIME_ROOT/runtime/site-packages" \
  PYTHONNOUSERSITE=1 \
  "$RUNTIME_ROOT/runtime/python/bin/python3.12" \
  "$RUNTIME_ROOT/runtime/moss_mlx_runner.py" --self-test |
  grep -q "moss-mlx-runtime ok"

if command -v otool >/dev/null 2>&1 &&
  file "$RUNTIME_ROOT/runtime/python/bin/python3.12" | grep -q 'Mach-O' &&
  otool -L "$RUNTIME_ROOT/runtime/python/bin/python3.12" |
  grep -E '/Users/runner|/opt/hostedtoolcache'; then
  echo "Packaged Python unexpectedly depends on the build host path" >&2
  exit 1
fi

if ! command -v ditto >/dev/null 2>&1; then
  echo "ditto is required to package the MOSS macOS runtime" >&2
  exit 1
fi

mkdir -p "$(dirname "$OUTPUT_ZIP")"
rm -f "$OUTPUT_ZIP" "$OUTPUT_ZIP.sha256"
COPYFILE_DISABLE=1 ditto -c -k --keepParent --norsrc --noextattr --noqtn --noacl \
  "$RUNTIME_ROOT" "$OUTPUT_ZIP"

archive_listing="$(unzip -Z1 "$OUTPUT_ZIP")"
if grep -E '(^|/)(\._|\.DS_Store|__MACOSX)' <<<"$archive_listing"; then
  echo "Packaged MOSS runtime contains macOS metadata sidecars" >&2
  exit 1
fi
if grep -E '\.safetensors$' <<<"$archive_listing"; then
  echo "Packaged MOSS runtime contains model weights that must be downloaded on demand" >&2
  exit 1
fi

archive_root="$(basename "$RUNTIME_ROOT")"
for relative in "${required_paths[@]}"; do
  if [[ -d "$RUNTIME_ROOT/$relative" ]]; then
    archive_entry_pattern="^$archive_root/$relative(/|$)"
  else
    archive_entry_pattern="^$archive_root/$relative$"
  fi
  if ! grep -Eq "$archive_entry_pattern" <<<"$archive_listing"; then
    echo "Packaged MOSS runtime is missing: $archive_root/$relative" >&2
    exit 1
  fi
done

(
  cd "$(dirname "$OUTPUT_ZIP")"
  shasum -a 256 "$(basename "$OUTPUT_ZIP")" >"$(basename "$OUTPUT_ZIP").sha256"
  shasum -a 256 --check "$(basename "$OUTPUT_ZIP").sha256"
)

echo "PASS: packaged MOSS runtime $(basename "$OUTPUT_ZIP")"
