#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

source "$REPO_ROOT/e2e-tests/test_utils/default_remote.sh"

internal_suffix="bytedance"
internal_suffix="${internal_suffix}.net"

echo "Checking tracked files for plaintext internal domains..."
if matches="$(git grep -I -n -i -F "$internal_suffix" -- .)"; then
    echo "ERROR: tracked files contain a plaintext internal domain suffix:" >&2
    printf '%s\n' "$matches" >&2
    exit 1
fi

required_fixture_hosts=(
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

decoded_default_url="$(bifrost_default_remote_base_url)"
if [[ "$decoded_default_url" != https://* ]]; then
    echo "ERROR: decoded default remote URL is not HTTPS" >&2
    exit 1
fi

for source_file in \
    crates/bifrost-storage/src/unified_config.rs \
    web/src/api/sync.ts \
    e2e-tests/test_utils/default_remote.sh; do
    if ! grep -q -F "$BIFROST_DEFAULT_REMOTE_HOST_BASE64" "$source_file"; then
        echo "ERROR: encoded default host is inconsistent in $source_file" >&2
        exit 1
    fi
done

echo "OK: no plaintext internal domains were found in tracked files."
echo "OK: the encoded default host decodes to an HTTPS runtime URL."
echo "OK: neutral example.com fixtures remain covered in Rust sources."
