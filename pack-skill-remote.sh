#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SRC="$SCRIPT_DIR/skill_remote.md"
OUT="$SCRIPT_DIR/bifrost-remote.zip"
TMP_DIR=$(mktemp -d)

trap 'rm -rf "$TMP_DIR"' EXIT

mkdir -p "$TMP_DIR/bifrost-remote"
cp "$SRC" "$TMP_DIR/bifrost-remote/SKILL.md"

(cd "$TMP_DIR" && zip -r "$OUT" bifrost-remote/)

echo "✅ Created $OUT"
