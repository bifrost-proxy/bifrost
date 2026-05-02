#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

echo "[skill-creator] running create -> test -> invoke -> delete -> import flow"
cargo run -p bifrost-e2e -- --category skill_creator --test skill_creator_create_test_invoke_delete_import --test-timeout 120
