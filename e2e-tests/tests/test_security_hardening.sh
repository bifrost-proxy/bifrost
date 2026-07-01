#!/bin/bash
#
# Security hardening regression suite for the 2026-07 audit findings.
# The checks are intentionally narrow and offline where possible.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

cd "$PROJECT_DIR"

run_step() {
    local label="$1"
    shift
    echo "==> $label"
    "$@"
}

run_step "C1 shell_text policy requires full regex match" \
    cargo test -p bifrost-admin shell_text_allow_pattern_requires_full_match --lib

run_step "C2 failed login threshold preserves password and remote state" \
    cargo test -p bifrost-admin failed_login_limit --lib

run_step "H1 script net.fetch rejects private targets by default" \
    cargo test -p bifrost-script net_fetch --lib

run_step "H2 virtual admin host peer=None is rejected without loopback trust" \
    cargo test -p bifrost-admin test_check_api_auth --lib

run_step "H3/H4 SSH key files are hardened and grant mode is preserved" \
    cargo test -p bifrost-admin remote_invoke::ssh_keys --lib

run_step "H5 installer rejects checksum mismatch and defaults to github.com only" \
    bash e2e-tests/tests/test_install_binary_adaptive_download.sh

run_step "M1 file glob does not follow symlink outside root" \
    cargo test -p bifrost-admin glob_does_not_follow_symlink_outside_root --lib

run_step "M2 encrypted remote command requires AAD and still decrypts valid payloads" \
    cargo test -p bifrost-admin decrypt_remote_command_payload --lib

run_step "M3 sync login rejects plain HTTP remote base URL" \
    cargo test -p bifrost-sync save_login_session_rejects_empty_or_invalid_input --lib

run_step "M4 unsafe SSL remains explicit opt-in" \
    cargo test -p bifrost-proxy unsafe_ssl --lib

run_step "Sync relay open-call accepts caller-provided encrypted call id" \
    pnpm --dir packages/bifrost-sync-server test -- --runInBand --testPathPattern remote-invoke

run_step "Web config surface type-checks with sandbox private-network toggle" \
    pnpm --dir web build

run_step "Functional regressions for hardened admin, remote invoke, sync, sandbox, and DevTools flows" \
    bash e2e-tests/tests/test_security_hardening_functional.sh
