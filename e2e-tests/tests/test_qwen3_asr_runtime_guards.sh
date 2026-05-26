#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

echo "[qwen3-asr-runtime-guards] syntax checks"
bash -n "$0"

echo "[qwen3-asr-runtime-guards] request timeout bounds"
cargo test -p bifrost-admin asr_runtime_timeouts_are_bounded_for_short_chunks --lib

echo "[qwen3-asr-runtime-guards] current chunk fork fallback reason"
cargo test -p bifrost-admin server_failure_recovery_reason_uses_fork_for_current_chunk --lib

echo "[qwen3-asr-runtime-guards] watchdog warning throttling"
cargo test -p bifrost-admin service_watchdog_warning_log_is_rate_limited --lib

echo "[qwen3-asr-runtime-guards] managed server breaker state"
cargo test -p bifrost-admin server_failure_breaker --lib

echo "[qwen3-asr-runtime-guards] strategy-level failure threshold"
cargo test -p bifrost-admin reuse_server_failure_threshold --lib

echo "[qwen3-asr-runtime-guards] done"
