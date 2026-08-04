#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

echo "[desktop-traffic-detail] validating native detail-window contract"

python3 - desktop/src-tauri/capabilities/default.json <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    capability = json.load(handle)

assert "host" in capability["windows"], capability
assert "traffic-detail" in capability["windows"], capability
PY

grep -Fq 'async fn open_traffic_detail_window' desktop/src-tauri/src/traffic_detail_window.rs
grep -Fq 'async fn close_traffic_detail_window' desktop/src-tauri/src/traffic_detail_window.rs
grep -Fq 'WebviewWindowBuilder::new(' desktop/src-tauri/src/traffic_detail_window.rs
grep -Fq 'get_webview(HOST_WINDOW_LABEL)' desktop/src-tauri/src/traffic_detail_window.rs
grep -Fq 'DESKTOP_TRAFFIC_DETAIL_CLOSED_EVENT' web/src/pages/Traffic/index.tsx

pnpm --dir web test:unit \
  src/pages/Traffic/detailWindow.test.ts \
  src/desktop/tauri.test.ts
pnpm --dir web run build:desktop

if [[ "$(uname -s)" == "Linux" ]] && {
  ! command -v pkg-config >/dev/null 2>&1 || ! pkg-config --exists glib-2.0 >/dev/null 2>&1;
}; then
  echo "[desktop-traffic-detail] SKIP Rust/Tauri: glib-2.0 metadata is unavailable"
  exit 0
fi

if ! compgen -G "$REPO_ROOT/desktop/src-tauri/resources/bin/*" >/dev/null; then
  cargo build -p bifrost-cli --bin bifrost
  node scripts/prepare-tauri-sidecar.mjs debug
fi

cargo test --manifest-path desktop/src-tauri/Cargo.toml traffic_detail -- --nocapture

echo "[desktop-traffic-detail] PASS"
