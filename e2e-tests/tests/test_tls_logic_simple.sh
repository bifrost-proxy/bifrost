#!/bin/bash

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

echo "=============================================="
echo "    TLS Intercept Logic Unit Test"
echo "=============================================="
echo ""

echo "Running should_intercept_tls unit tests..."
cargo test --package bifrost-proxy test_should_intercept -- --nocapture 2>&1

echo ""
echo "Running E2E TLS mode tests..."
cargo test --package bifrost-e2e tls_intercept_mode -- --nocapture 2>&1

echo ""
echo "=============================================="
echo "    All TLS Logic Tests Complete"
echo "=============================================="
