#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

internal_suffix="bytedance"
internal_suffix="${internal_suffix}.net"
allowed_internal_host="bifrost.${internal_suffix}"
escaped_internal_suffix="${internal_suffix//./\\.}"

echo "Checking tracked files against the internal-domain allowlist..."
unexpected_hosts=""
while IFS= read -r host; do
    [[ -z "$host" ]] && continue
    normalized_host="$(printf '%s' "$host" | tr '[:upper:]' '[:lower:]')"
    if [[ "$normalized_host" != "$allowed_internal_host" ]]; then
        unexpected_hosts="${unexpected_hosts}${host}"$'\n'
    fi
done < <(git grep -I -h -o -i -E "([[:alnum:]_-]+\\.)*${escaped_internal_suffix}" -- . | sort -fu || true)

if [[ -n "$unexpected_hosts" ]]; then
    echo "ERROR: tracked files contain internal domains outside the allowlist:" >&2
    printf '%s' "$unexpected_hosts" >&2
    exit 1
fi

required_fixture_hosts=(
    "$allowed_internal_host"
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

echo "OK: the required login host is the only allowlisted internal domain."
echo "OK: neutral example.com fixtures remain covered in Rust sources."
