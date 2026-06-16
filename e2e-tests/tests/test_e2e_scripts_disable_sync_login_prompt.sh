#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

python3 - <<'PY'
from pathlib import Path

ROOTS = [
    Path("e2e-tests"),
    Path("scripts"),
    Path("tests"),
]
START_HINTS = [
    "BIFROST_BIN",
    "target/debug/bifrost",
    "target/release/bifrost",
    "cargo run --bin bifrost",
    "cargo run --release --bin bifrost",
    '"$BIN" start',
    '"$BIFROST_BIN" start',
    "$BIFROST_BIN start",
    "./target/debug/bifrost start",
    "./target/release/bifrost start",
]
ALLOWLIST = {
    "e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh",
}

missing = []
missing_tray = []
for root in ROOTS:
    for path in sorted(root.rglob("*.sh")):
        rel = path.as_posix()
        if rel in ALLOWLIST:
            continue
        text = path.read_text(errors="ignore")
        if "start" not in text:
            continue
        if not any(hint in text for hint in START_HINTS):
            continue
        if (
            "BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT" not in text
            and "process.sh" not in text
            and "admin_client.sh" not in text
        ):
            missing.append(rel)
        if (
            "BIFROST_DISABLE_TRAY" not in text
            and "--no-tray" not in text
            and "process.sh" not in text
            and "admin_client.sh" not in text
        ):
            missing_tray.append(rel)

process_helper = Path("e2e-tests/test_utils/process.sh").read_text(errors="ignore")
if "BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1" not in process_helper:
    missing.append("e2e-tests/test_utils/process.sh")
if "BIFROST_DISABLE_TRAY:=1" not in process_helper:
    missing_tray.append("e2e-tests/test_utils/process.sh")

rust_missing = []
for path in sorted(Path("crates").rglob("*.rs")) + sorted(Path("tests").rglob("*.rs")):
    text = path.read_text(errors="ignore")
    if 'CARGO_BIN_EXE_bifrost' not in text:
        continue
    if ".arg(\"start\")" not in text:
        continue
    if "BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT" not in text:
        rust_missing.append(path.as_posix())

missing.extend(rust_missing)

if missing or missing_tray:
    if missing:
        print("Bifrost startup tests/scripts that may open the Sync login prompt:")
        for item in missing:
            print(f"  - {item}")
    if missing_tray:
        print("Bifrost startup tests/scripts that may spawn a tray helper:")
        for item in missing_tray:
            print(f"  - {item}")
    raise SystemExit(1)

print("All Bifrost startup tests/scripts disable desktop UI by default.")
PY
