#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

TAURI_CONFIG="desktop/src-tauri/tauri.conf.json"
DESKTOP_CARGO="desktop/src-tauri/Cargo.toml"
TRAY_MENU="crates/bifrost-cli/src/commands/tray/menu.rs"
TRAY_RUNTIME="crates/bifrost-cli/src/commands/tray/tray.rs"

python3 - "$TAURI_CONFIG" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, "r", encoding="utf-8") as handle:
    config = json.load(handle)

schemes = (
    config.get("plugins", {})
    .get("deep-link", {})
    .get("desktop", {})
    .get("schemes", [])
)
assert "bifrost" in schemes, schemes

associations = config.get("bundle", {}).get("fileAssociations", [])
assert any("bifrost" in item.get("ext", []) for item in associations), associations

association = next(item for item in associations if "bifrost" in item.get("ext", []))
assert association.get("role") == "Editor", association
assert association.get("mimeType") == "application/x-bifrost", association
PY

grep -q 'tauri-plugin-deep-link' "$DESKTOP_CARGO"
grep -q 'tauri-plugin-single-instance' "$DESKTOP_CARGO"
grep -q 'OpenAppRoute' "$TRAY_MENU"
grep -q 'bifrost://open/' "$TRAY_RUNTIME"
grep -q 'fallback_url' "$TRAY_RUNTIME"

cargo test -p bifrost-cli tray::
cargo test --manifest-path desktop/src-tauri/Cargo.toml open_requests

echo "desktop open request contract passed"
