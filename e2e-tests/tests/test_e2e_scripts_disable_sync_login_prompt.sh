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

process_helper = Path("e2e-tests/test_utils/process.sh").read_text(errors="ignore")
if "BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1" not in process_helper:
    missing.append("e2e-tests/test_utils/process.sh")

if missing:
    print("Scripts that may start Bifrost without disabling Sync login prompt:")
    for item in missing:
        print(f"  - {item}")
    raise SystemExit(1)

print("All E2E Bifrost startup scripts disable Sync auto-login prompt by default.")
PY
