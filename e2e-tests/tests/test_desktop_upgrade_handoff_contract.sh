#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_ROOT"

echo "[desktop-upgrade-handoff] running desktop handoff contract tests"

if [[ "$(uname -s)" == "Linux" ]]; then
  if ! command -v pkg-config >/dev/null 2>&1 || ! pkg-config --exists glib-2.0 >/dev/null 2>&1; then
    echo "[desktop-upgrade-handoff] SKIP: glib-2.0 pkg-config metadata is not available on this Linux runner"
    echo "[desktop-upgrade-handoff]       desktop/src-tauri depends on Tauri's GTK stack on Linux; macOS/desktop-capable runners execute the contract"
    exit 0
  fi
fi

if ! compgen -G "$REPO_ROOT/desktop/src-tauri/resources/bin/*" >/dev/null; then
  echo "[desktop-upgrade-handoff] SKIP: desktop sidecar resources are not prepared"
  echo "[desktop-upgrade-handoff]       run this contract in a desktop-capable build environment with desktop/src-tauri/resources/bin populated"
  exit 0
fi

CARGO_TARGET_DIR="$REPO_ROOT/target/desktop-upgrade-handoff-contract" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml upgrade_relaunch -- --nocapture
CARGO_TARGET_DIR="$REPO_ROOT/target/desktop-upgrade-handoff-contract" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml restart_handoff_setup_failure -- --nocapture
CARGO_TARGET_DIR="$REPO_ROOT/target/desktop-upgrade-handoff-contract" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml deferred_desktop_installer_marker -- --nocapture
CARGO_TARGET_DIR="$REPO_ROOT/target/desktop-upgrade-handoff-contract" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml deferred_desktop_install_completion -- --nocapture
CARGO_TARGET_DIR="$REPO_ROOT/target/desktop-upgrade-handoff-contract" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml deferred_desktop_install_commit -- --nocapture

DESKTOP_MAIN="$REPO_ROOT/desktop/src-tauri/src/main.rs"
DESKTOP_HANDOFF="$REPO_ROOT/desktop/src-tauri/src/upgrade_handoff.rs"
DESKTOP_BACKEND="$REPO_ROOT/desktop/src-tauri/src/backend_runtime.rs"
DESKTOP_TESTS="$REPO_ROOT/desktop/src-tauri/src/tests.rs"

for module in "$DESKTOP_MAIN" "$DESKTOP_HANDOFF" "$DESKTOP_BACKEND" "$DESKTOP_TESTS"; do
  if [[ "$(wc -l <"$module")" -gt 1500 ]]; then
    echo "[desktop-upgrade-handoff] FAIL: desktop module exceeds 1500 lines: $module"
    exit 1
  fi
done

if ! grep -Fq 'sanitize_desktop_upgrade_relaunch_command(&mut command)' "$DESKTOP_HANDOFF"; then
  echo "[desktop-upgrade-handoff] FAIL: relaunch path does not sanitize helper-only environment"
  exit 1
fi

if ! grep -Fq 'persist_desktop_upgrade_handoff_failure(' "$DESKTOP_HANDOFF"; then
  echo "[desktop-upgrade-handoff] FAIL: relaunch setup errors are not persisted to shared progress"
  exit 1
fi

if ! grep -Fq 'spawn_windows_desktop_upgrade_handoff(marker_path, target)' "$DESKTOP_HANDOFF"; then
  echo "[desktop-upgrade-handoff] FAIL: deferred Windows installer is not owned by the post-exit handoff helper"
  exit 1
fi

if ! grep -Fq 'package_owned_by_updater' "$DESKTOP_MAIN"; then
  echo "[desktop-upgrade-handoff] FAIL: deferred handoff cannot distinguish updater downloads from caller-owned packages"
  exit 1
fi

if ! grep -Fq '$rollback = New-InstallSnapshot $marker' "$DESKTOP_HANDOFF" ||
  ! grep -Fq '$terminal = Wait-ForDesktopVerification $startedApp' "$DESKTOP_HANDOFF" ||
  ! grep -Fq 'Restore-InstallSnapshot $rollback' "$DESKTOP_HANDOFF"; then
  echo "[desktop-upgrade-handoff] FAIL: deferred Windows install is not a verified rollback transaction"
  exit 1
fi

if ! grep -Fq 'request_desktop_shutdown(app)' "$DESKTOP_BACKEND"; then
  echo "[desktop-upgrade-handoff] FAIL: failed relaunched App/core cannot release Windows files for rollback"
  exit 1
fi

echo "[desktop-upgrade-handoff] PASS"
