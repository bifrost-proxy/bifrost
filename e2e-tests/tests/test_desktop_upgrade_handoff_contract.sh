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

CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml upgrade_relaunch -- --nocapture
CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml restart_handoff_setup_failure -- --nocapture
CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml deferred_desktop_installer_marker -- --nocapture
CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml deferred_desktop_install_completion -- --nocapture
CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml deferred_desktop_completion_preserves_transaction_artifacts_for_helper_commit -- --nocapture
CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml cli_owned_upgrade_relaunch_reuses_the_target_backend_even_when_pid_is_unchanged -- --nocapture
CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml cli_owned_upgrade_relaunch_refuses_takeover_when_port_is_free -- --nocapture
CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml cli_owned_upgrade_relaunch_keeps_refusing_when_port_is_still_occupied -- --nocapture
CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml failed_cli_owned_handoff_retries_without_another_thirty_second_wait -- --nocapture
CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml healthy_target_backend_completes_and_clears_cli_upgrade_handoff -- --nocapture
CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml healthy_wrong_version_backend_does_not_bypass_cli_upgrade_handoff -- --nocapture
CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml healthy_target_backend_on_another_port_does_not_complete_cli_upgrade_handoff -- --nocapture
CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_shutdown_stops_only_a_backend_owned_by_the_desktop -- --nocapture
CARGO_TARGET_DIR="${BIFROST_DESKTOP_TEST_TARGET_DIR:-$REPO_ROOT/target/desktop-upgrade-handoff-contract}" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml cli_owned_upgrade_relaunch_preserves_wrong_version_core_owned_by_same_data_dir -- --nocapture

DESKTOP_MAIN="$REPO_ROOT/desktop/src-tauri/src/main.rs"
DESKTOP_HANDOFF="$REPO_ROOT/desktop/src-tauri/src/upgrade_handoff.rs"
DESKTOP_BACKEND="$REPO_ROOT/desktop/src-tauri/src/backend_runtime.rs"
DESKTOP_RUNTIME="$REPO_ROOT/desktop/src-tauri/src/runtime_ownership.rs"
DESKTOP_TESTS="$REPO_ROOT/desktop/src-tauri/src/tests.rs"
DESKTOP_RECOVERY_TESTS="$REPO_ROOT/desktop/src-tauri/src/tests/cli_handoff_recovery.rs"

for module in "$DESKTOP_MAIN" "$DESKTOP_HANDOFF" "$DESKTOP_BACKEND" "$DESKTOP_RUNTIME" "$DESKTOP_TESTS" "$DESKTOP_RECOVERY_TESTS"; do
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

if grep -Fq 'commit_deferred_desktop_install_artifacts' "$DESKTOP_BACKEND" ||
  ! grep -Fq '$terminal = Wait-ForDesktopVerification $startedApp' "$DESKTOP_HANDOFF" ||
  ! grep -Fq 'Remove-InstallSnapshot $rollback' "$DESKTOP_HANDOFF"; then
  echo "[desktop-upgrade-handoff] FAIL: the relaunched App can clean rollback artifacts before the helper observes Completed"
  exit 1
fi

if ! grep -Fq 'resolve_external_cli_backend_handoff(data_dir, marker, effective_wait)' "$DESKTOP_BACKEND" ||
  ! grep -Fq 'wait_for_external_cli_backend(data_dir, marker, timeout)' "$DESKTOP_HANDOFF" ||
  ! grep -Fq 'refusing to launch a desktop-managed core' "$DESKTOP_HANDOFF" ||
  ! grep -Fq 'observed_external_core_pid' "$DESKTOP_HANDOFF" ||
  ! grep -Fq 'fn is_port_available(port: u16) -> bool' "$DESKTOP_RUNTIME" ||
  ! grep -Fq 'TcpListener::bind((BACKEND_BIND_HOST, port)).is_ok()' "$DESKTOP_RUNTIME"; then
  echo "[desktop-upgrade-handoff] FAIL: CLI-owned WebView relaunch ownership/fail-closed contract is missing"
  exit 1
fi

if ! grep -Fq 'desktop_shutdown_backend_action(' "$DESKTOP_RUNTIME" ||
  ! grep -Fq 'DesktopShutdownBackendAction::PreserveExternalRuntime' "$DESKTOP_RUNTIME" ||
  ! grep -Fq 'desktop_shutdown_backend_action_for_state(&state)' "$DESKTOP_MAIN" ||
  ! grep -Fq 'upgrade_handoff_requires_backend_release(marker)' "$DESKTOP_HANDOFF"; then
  echo "[desktop-upgrade-handoff] FAIL: desktop shutdown/runtime ownership contract is missing"
  exit 1
fi

echo "[desktop-upgrade-handoff] PASS"
