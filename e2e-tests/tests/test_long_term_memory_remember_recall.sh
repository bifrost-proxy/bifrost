#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_DIR"

echo "[long-term-memory-e2e] RUN /remember -> new session recall injection"
cargo run -p bifrost-e2e -- \
  --test im_gateway_agent_long_term_memory_remember_recall \
  --test-timeout 120 \
  --port 18880
echo "[long-term-memory-e2e] PASS /remember -> new session recall injection"
