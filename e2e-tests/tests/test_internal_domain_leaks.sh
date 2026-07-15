#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

internal_suffix="bytedance"
internal_suffix="${internal_suffix}.net"

echo "Checking tracked files for internal domain references..."
if matches="$(git grep -I -n -i -F "$internal_suffix" -- .)"; then
    echo "ERROR: tracked files contain a restricted internal domain suffix:" >&2
    echo "$matches" >&2
    exit 1
fi

required_fixture_hosts=(
    "bifrost.example.com"
    "api.example.com"
    "app.example.com"
    "cdn.example.com"
)

for host in "${required_fixture_hosts[@]}"; do
    if ! git grep -I -q -F "$host" -- crates; then
        echo "ERROR: expected neutral fixture host is missing from Rust sources: $host" >&2
        exit 1
    fi
done

echo "OK: no restricted internal domains were found in tracked files."
echo "OK: neutral example.com fixtures remain covered in Rust sources."
